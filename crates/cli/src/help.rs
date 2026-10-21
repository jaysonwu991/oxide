//! Localize Clap's display metadata without changing argument or command IDs.
use crate::i18n::{t, Language, CATALOG};
use clap::{Arg, ArgAction, Command, CommandFactory};
use std::ffi::OsString;

fn normalized(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches('.')
        .to_string()
}

fn translated(lang: Language, source: &str) -> Option<&'static str> {
    let source = normalized(source);
    CATALOG
        .iter()
        .find(|(key, en, _)| key.starts_with("cli.help.") && normalized(en) == source)
        .map(|(_, en, zh)| if lang == Language::ZhCn { *zh } else { *en })
}

// Probe with the real argument grammar so flag-looking option values and prompt
// text cannot become language/cwd overrides. Help must not exit this first pass.
fn help_probe(command: Command) -> Command {
    command
        .disable_help_flag(true)
        .disable_version_flag(true)
        .disable_help_subcommand(true)
        .arg(
            Arg::new("help")
                .long("help")
                .short('h')
                .action(ArgAction::SetTrue),
        )
        .ignore_errors(true)
        .mut_subcommands(help_probe)
}

pub fn language(args: &[OsString], cwd: &std::path::Path) -> Language {
    let matches = help_probe(crate::Cli::command())
        .try_get_matches_from(args)
        .ok();
    if let Some(language) = matches
        .as_ref()
        .and_then(|matches| matches.get_one::<String>("language"))
        .and_then(|tag| Language::parse(tag))
    {
        return language;
    }
    let effective_cwd = matches
        .as_ref()
        .and_then(|matches| matches.get_one::<std::path::PathBuf>("cwd"))
        .map(|path| cwd.join(path))
        .unwrap_or_else(|| cwd.to_path_buf());
    crate::i18n::resolve(&effective_cwd)
}

pub fn command(mut command: Command, lang: Language) -> Command {
    if lang != Language::ZhCn {
        return command;
    }
    // Materialize Clap's generated help/version arguments before translating.
    command.build();
    if let Some(text) = command
        .get_about()
        .and_then(|text| translated(lang, &text.to_string()))
    {
        command = command.about(text);
    }
    if let Some(text) = command
        .get_long_about()
        .and_then(|text| translated(lang, &text.to_string()))
    {
        command = command.long_about(text);
    }
    command
        .help_template(t(lang, "cli.help.layout"))
        .subcommand_help_heading(t(lang, "cli.help.commandHeading"))
        .mut_args(|mut arg| {
            if let Some(text) = arg
                .get_help()
                .and_then(|text| translated(lang, &text.to_string()))
            {
                arg = arg.help(text);
            }
            if let Some(text) = arg
                .get_long_help()
                .and_then(|text| translated(lang, &text.to_string()))
            {
                arg = arg.long_help(text);
            }
            let heading = if arg.is_positional() {
                t(lang, "cli.help.argumentHeading")
            } else {
                t(lang, "cli.help.optionHeading")
            };
            arg.help_heading(heading)
        })
        .mut_subcommands(|sub| self::command(sub, lang))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, FromArgMatches, Parser};

    fn check_help(command: &Command) {
        for text in [command.get_about(), command.get_long_about()]
            .into_iter()
            .flatten()
        {
            assert!(
                translated(Language::ZhCn, &text.to_string()).is_some(),
                "untranslated command help: {text}"
            );
        }
        for arg in command.get_arguments() {
            for text in [arg.get_help(), arg.get_long_help()].into_iter().flatten() {
                assert!(
                    translated(Language::ZhCn, &text.to_string()).is_some(),
                    "untranslated argument help: {text}"
                );
            }
        }
        for sub in command.get_subcommands() {
            check_help(sub);
        }
    }

    #[test]
    fn every_cli_help_description_has_a_translation() {
        let mut cli = crate::Cli::command();
        cli.build();
        check_help(&cli);
    }

    #[test]
    fn localized_help_keeps_command_and_argument_identifiers() {
        let mut cli = command(crate::Cli::command(), Language::ZhCn);
        let help = cli.render_long_help().to_string();
        assert!(help.contains("用法："));
        assert!(help.contains("使用的模型"));
        assert!(help.contains("--ask-approvals"));
        let args = ["oxide", "--model", "test", "sessions", "list", "--json"];
        let translated =
            crate::Cli::from_arg_matches(&cli.try_get_matches_from(args).unwrap()).unwrap();
        let original = crate::Cli::try_parse_from(args).unwrap();
        assert_eq!(format!("{translated:?}"), format!("{original:?}"));
        let matches = command(crate::Cli::command(), Language::ZhCn)
            .try_get_matches_from(["oxide", "sessions", "list", "--json"])
            .unwrap();
        assert!(matches!(
            crate::Cli::from_arg_matches(&matches).unwrap().command,
            Some(crate::Command::Sessions { .. })
        ));
    }

    #[test]
    fn language_flag_is_read_before_help_exits() {
        for flag in [
            vec!["oxide", "--language", "zh-CN", "--help"],
            vec!["oxide", "--language=zh-CN", "--help"],
        ] {
            let args = flag.iter().map(OsString::from).collect::<Vec<_>>();
            assert_eq!(language(&args, std::path::Path::new(".")), Language::ZhCn);
        }
    }
}

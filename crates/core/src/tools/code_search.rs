use super::{resolve, spec as tool_spec, strict_int_arg, NO_MATCHES};
use crate::llm::ToolSpec;
use anyhow::{Context, Result};
use model2vec_rs::model::StaticModel;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const MODEL: &str = "minishlab/potion-code-16M-v2";
const REVISION: &str = "e9d2a44ca6a05ac6685f3b23709ea57eb7352d5b";
const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const MAX_CHUNKS: usize = 50_000;
const CHUNK_LINES: usize = 60;
const CHUNK_BYTES: usize = 6000;
const INDEX_VERSION: u32 = 1;

pub(super) fn spec(scope: &str) -> ToolSpec {
    tool_spec(
        "code_search",
        &format!("Find relevant code snippets using native local semantic search plus BM25. Use natural-language questions to discover implementations; use grep for exact matches or exhaustive references and read for surrounding context. Results are ranked, not exhaustive. Rust, JavaScript, TypeScript and Python use syntax-aware chunks; other files use overlapping text chunks. Changed files are reindexed on every call. First use downloads public embedding weights (about 34 MB); no source code is sent over the network. Requires network only for the initial model download, or OXIDE_CODE_SEARCH_MODEL pointing to a local Model2Vec directory.{scope}"),
        json!({"type":"object","properties":{
            "query":{"type":"string","description":"Natural-language question or code to search for"},
            "path":{"type":"string","description":"Local directory to index (default: .). For large repositories, use find or grep to locate the relevant subdirectory first, then set path to it; indexing the whole repository may exceed the scope limits."},
            "limit":{"type":"integer","minimum":1,"maximum":20,"description":"Maximum ranked snippets (default: 5)"},
            "content":{"type":"string","enum":["code","docs","config","all"],"description":"Files to search (default: code)"},
            "max_snippet_lines":{"type":"integer","minimum":0,"maximum":200,"description":"Lines per snippet (default: 80; 0 returns locations only)"}
        },"required":["query"]}),
    )
}

struct Request {
    root: PathBuf,
    query: String,
    content: String,
    limit: usize,
    lines: usize,
}

impl Request {
    fn parse(cwd: &Path, args: &Value) -> Result<Self> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .context("code_search requires a `query` string")?
            .trim();
        anyhow::ensure!(!query.is_empty(), "query must not be empty");
        anyhow::ensure!(query.len() <= 8192, "query must be at most 8192 bytes");
        let limit = strict_int_arg(args, "limit")?.unwrap_or(5);
        anyhow::ensure!((1..=20).contains(&limit), "limit must be between 1 and 20");
        let lines = strict_int_arg(args, "max_snippet_lines")?.unwrap_or(80);
        anyhow::ensure!(lines <= 200, "max_snippet_lines must be between 0 and 200");
        let content = match args.get("content") {
            None => "code",
            Some(value) => value.as_str().context("content must be a string")?,
        };
        anyhow::ensure!(
            matches!(content, "code" | "docs" | "config" | "all"),
            "unknown content type: {content}"
        );
        let path = match args.get("path") {
            None => ".",
            Some(value) => value
                .as_str()
                .context("path must be a local directory string")?,
        };
        let root = std::fs::canonicalize(resolve(cwd, path))
            .with_context(|| format!("unable to access local directory `{path}`"))?;
        anyhow::ensure!(root.is_dir(), "code_search path must be a local directory");
        Ok(Self {
            root,
            query: query.to_owned(),
            content: content.to_owned(),
            limit,
            lines,
        })
    }
}

fn cache_root() -> Result<PathBuf> {
    Ok(dirs::cache_dir()
        .context("unable to locate the OS cache directory")?
        .join("oxide/code-search"))
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("cache path has no parent")?;
    std::fs::create_dir_all(parent).context("unable to create code search cache")?;
    let mut nonce = [0u8; 8];
    getrandom::getrandom(&mut nonce).map_err(|error| anyhow::anyhow!("cache nonce: {error}"))?;
    let temporary = parent.join(format!(".{}.tmp", hash(&nonce)));
    let result = (|| {
        std::fs::write(&temporary, bytes)?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

async fn model_directory() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("OXIDE_CODE_SEARCH_MODEL") {
        return std::fs::canonicalize(&path)
            .context("OXIDE_CODE_SEARCH_MODEL must name a local Model2Vec directory");
    }
    let directory = cache_root()?.join("models").join(REVISION);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?;
    for (name, maximum) in [
        ("config.json", 1024 * 1024),
        ("tokenizer.json", 16 * 1024 * 1024),
        ("model.safetensors", 128 * 1024 * 1024),
    ] {
        let path = directory.join(name);
        if path.is_file() {
            continue;
        }
        let url = format!("https://huggingface.co/{MODEL}/resolve/{REVISION}/{name}");
        let mut response = client.get(url).send().await
            .context("embedding model download failed; retry with network access or set OXIDE_CODE_SEARCH_MODEL to a local model; grep and read remain available")?
            .error_for_status().context("embedding model download rejected")?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .context("embedding model download interrupted")?
        {
            anyhow::ensure!(
                bytes.len() + chunk.len() <= maximum,
                "embedding model file exceeds download limit: {name}"
            );
            bytes.extend_from_slice(&chunk);
        }
        atomic_write(&path, &bytes).with_context(|| format!("unable to cache {name}"))?;
    }
    Ok(directory)
}

fn load_model(directory: &Path) -> Result<(String, Arc<StaticModel>)> {
    struct Loaded {
        directory: PathBuf,
        stamps: Vec<(u64, std::time::SystemTime)>,
        key: String,
        model: Arc<StaticModel>,
    }
    static MODEL_CACHE: OnceLock<Mutex<Option<Loaded>>> = OnceLock::new();
    let assets = ["config.json", "tokenizer.json", "model.safetensors"];
    let stamps = assets
        .iter()
        .map(|name| {
            let metadata = std::fs::metadata(directory.join(name))
                .with_context(|| format!("unable to inspect model asset {name}"))?;
            Ok((metadata.len(), metadata.modified()?))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut cached = MODEL_CACHE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| anyhow::anyhow!("embedding model cache lock poisoned"))?;
    if let Some(loaded) = cached
        .as_ref()
        .filter(|loaded| loaded.directory == directory && loaded.stamps == stamps)
    {
        return Ok((loaded.key.clone(), Arc::clone(&loaded.model)));
    }
    // Hash assets only when they change; rehashing 34 MB per query dominates warm searches.
    let mut fingerprint = Sha256::new();
    for name in assets {
        fingerprint.update(
            std::fs::read(directory.join(name))
                .with_context(|| format!("unable to read model asset {name}"))?,
        );
    }
    let key = format!("{:x}", fingerprint.finalize());
    let model = Arc::new(StaticModel::from_pretrained(directory, None, Some(true), None)
        .context("unable to load native Model2Vec weights; check OXIDE_CODE_SEARCH_MODEL or remove the cached model to download again")?);
    *cached = Some(Loaded {
        directory: directory.to_path_buf(),
        stamps,
        key: key.clone(),
        model: Arc::clone(&model),
    });
    Ok((key, model))
}

#[derive(Serialize, Deserialize)]
struct Chunk {
    start: usize,
    end: usize,
    text: String,
    vector: Vec<f32>,
}

#[derive(Serialize, Deserialize)]
struct IndexedFile {
    digest: String,
    chunks: Vec<Chunk>,
}

#[derive(Default, Serialize, Deserialize)]
struct Index {
    version: u32,
    model: String,
    files: BTreeMap<String, IndexedFile>,
}

fn content_kind(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "rs" | "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "py" | "pyi" | "go" | "java"
        | "kt" | "kts" | "c" | "h" | "cpp" | "hpp" | "cc" | "cs" | "rb" | "php" | "swift" | "m"
        | "mm" | "scala" | "sh" | "bash" | "zsh" | "sql" | "vue" | "svelte" | "html" | "css"
        | "scss" | "lua" | "ex" | "exs" | "erl" | "hs" | "r" | "jl" | "dart" | "zig" | "proto" => {
            "code"
        }
        "md" | "mdx" | "rst" | "txt" | "adoc" => "docs",
        "toml" | "yaml" | "yml" | "json" | "jsonc" | "ini" | "cfg" | "xml" | "env" => "config",
        _ => return None,
    })
}

fn language(path: &Path) -> Option<tree_sitter::Language> {
    Some(match path.extension()?.to_str()? {
        "rs" => tree_sitter_rust::LANGUAGE.into(),
        "js" | "jsx" | "mjs" | "cjs" => tree_sitter_javascript::LANGUAGE.into(),
        "ts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "py" | "pyi" => tree_sitter_python::LANGUAGE.into(),
        _ => return None,
    })
}

fn chunks(path: &Path, text: &str) -> Vec<Chunk> {
    let lines: Vec<_> = text.lines().collect();
    let mut boundaries = BTreeSet::from([0, lines.len()]);
    if let Some(language) = language(path) {
        let mut parser = tree_sitter::Parser::new();
        if parser.set_language(&language).is_ok() {
            if let Some(tree) = parser.parse(text, None) {
                let mut pending = vec![tree.root_node()];
                while let Some(node) = pending.pop() {
                    if matches!(
                        node.kind(),
                        "function_item"
                            | "impl_item"
                            | "struct_item"
                            | "enum_item"
                            | "trait_item"
                            | "function_definition"
                            | "class_definition"
                            | "decorated_definition"
                            | "function_declaration"
                            | "class_declaration"
                            | "method_definition"
                            | "export_statement"
                            | "lexical_declaration"
                            | "interface_declaration"
                            | "type_alias_declaration"
                    ) {
                        boundaries.insert(node.start_position().row);
                        boundaries.insert((node.end_position().row + 1).min(lines.len()));
                    }
                    let mut cursor = node.walk();
                    pending.extend(node.named_children(&mut cursor));
                }
            }
        }
    }
    let boundaries: Vec<_> = boundaries.into_iter().collect();
    let mut out = Vec::new();
    for pair in boundaries.windows(2) {
        let mut start = pair[0];
        while start < pair[1] {
            let mut end = start;
            let mut bytes = 0;
            while end < pair[1] && end - start < CHUNK_LINES {
                let size = lines[end].len() + 1;
                if end > start && bytes + size > CHUNK_BYTES {
                    break;
                }
                bytes += size;
                end += 1;
            }
            // Very long lines (generated/minified files) are not useful retrieval units.
            if bytes <= CHUNK_BYTES && lines[start..end].iter().any(|line| !line.trim().is_empty())
            {
                out.push(Chunk {
                    start: start + 1,
                    end,
                    text: lines[start..end].join("\n"),
                    vector: Vec::new(),
                });
            }
            start = if end < pair[1] && end - start > 10 {
                end - 5
            } else {
                end
            };
        }
    }
    out
}

fn refresh(
    index: &mut Index,
    request: &Request,
    mut encode: impl FnMut(&str) -> Vec<f32>,
) -> Result<(bool, Vec<String>)> {
    let mut builder = ignore::WalkBuilder::new(&request.root);
    builder
        .hidden(false)
        .require_git(false)
        .follow_links(false)
        .add_custom_ignore_filename(".sembleignore")
        .filter_entry(|entry| {
            !matches!(
                entry.file_name().to_str(),
                Some(
                    ".git"
                        | "target"
                        | "node_modules"
                        | ".venv"
                        | "venv"
                        | "dist"
                        | "build"
                        | "__pycache__"
                )
            )
        });
    let mut seen = BTreeSet::new();
    let mut warnings = Vec::new();
    let mut changed = false;
    let mut source_bytes = 0;
    let mut chunk_count = 0;
    for entry in builder.build() {
        let entry = entry.context("unable to walk code search directory")?;
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        let Some(kind) = content_kind(path) else {
            continue;
        };
        if request.content != "all" && request.content != kind {
            continue;
        }
        let name = path
            .strip_prefix(&request.root)?
            .to_string_lossy()
            .replace('\\', "/");
        let metadata = entry
            .metadata()
            .with_context(|| format!("unable to inspect {name}"))?;
        if metadata.len() > MAX_FILE_BYTES {
            warnings.push(format!("skipped {name}: larger than 1 MiB"));
            continue;
        }
        let bytes = std::fs::read(path).with_context(|| format!("unable to read {name}"))?;
        source_bytes += bytes.len();
        anyhow::ensure!(
            source_bytes <= MAX_SOURCE_BYTES,
            "code_search scope exceeds 64 MiB; select a narrower path"
        );
        if bytes.contains(&0) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let digest = hash(&bytes);
        if !index
            .files
            .get(&name)
            .is_some_and(|file| file.digest == digest)
        {
            let mut chunks = chunks(path, text);
            for chunk in &mut chunks {
                chunk.vector = encode(&format!("{name}\n{}", chunk.text));
            }
            index
                .files
                .insert(name.clone(), IndexedFile { digest, chunks });
            changed = true;
        }
        chunk_count += index.files[&name].chunks.len();
        anyhow::ensure!(
            chunk_count <= MAX_CHUNKS,
            "code_search scope exceeds 50000 chunks; select a narrower path"
        );
        seen.insert(name);
    }
    let old = index.files.len();
    index.files.retain(|name, _| seen.contains(name));
    Ok((changed || index.files.len() != old, warnings))
}

fn terms(text: &str) -> Vec<String> {
    let mut separated = String::with_capacity(text.len());
    let mut previous = None;
    for character in text.chars() {
        if character.is_uppercase() && previous.is_some_and(char::is_lowercase) {
            separated.push(' ');
        }
        separated.push(if character.is_alphanumeric() {
            character
        } else {
            ' '
        });
        previous = Some(character);
    }
    separated
        .split_whitespace()
        .map(str::to_lowercase)
        .collect()
}

fn ranked<'a>(index: &'a Index, query: &str, vector: &[f32]) -> Vec<(&'a str, &'a Chunk)> {
    let documents: Vec<_> = index
        .files
        .iter()
        .flat_map(|(path, file)| file.chunks.iter().map(move |chunk| (path.as_str(), chunk)))
        .collect();
    if documents.is_empty() {
        return Vec::new();
    }
    let query_terms: BTreeSet<_> = terms(query).into_iter().collect();
    let counts: Vec<_> = documents
        .iter()
        .map(|(path, chunk)| {
            let mut counts = HashMap::<String, usize>::new();
            for term in terms(&format!("{path}\n{}", chunk.text)) {
                *counts.entry(term).or_default() += 1;
            }
            counts
        })
        .collect();
    let lengths: Vec<usize> = counts.iter().map(|counts| counts.values().sum()).collect();
    let average = (lengths.iter().sum::<usize>() as f32 / documents.len() as f32).max(1.0);
    let mut lexical = vec![0.0f32; documents.len()];
    for term in query_terms {
        let frequency = counts
            .iter()
            .filter(|counts| counts.contains_key(&term))
            .count() as f32;
        let idf = (1.0 + (documents.len() as f32 - frequency + 0.5) / (frequency + 0.5)).ln();
        for (i, counts) in counts.iter().enumerate() {
            let tf = *counts.get(&term).unwrap_or(&0) as f32;
            lexical[i] += idf * tf * 2.2 / (tf + 1.2 * (0.25 + 0.75 * lengths[i] as f32 / average));
        }
    }
    let semantic: Vec<f32> = documents
        .iter()
        .map(|(_, chunk)| chunk.vector.iter().zip(vector).map(|(a, b)| a * b).sum())
        .collect();
    let mut fused = vec![0.0f32; documents.len()];
    let symbol_query =
        query.split_whitespace().count() == 1 || query.contains("::") || query.contains('_');
    for (scores, weight) in [
        (&lexical, if symbol_query { 2.0 } else { 1.0 }),
        (&semantic, 1.0),
    ] {
        let mut order: Vec<_> = (0..documents.len()).filter(|&i| scores[i] > 0.0).collect();
        order.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]).then(a.cmp(&b)));
        for (rank, i) in order.into_iter().take(100).enumerate() {
            fused[i] += weight / (60 + rank + 1) as f32;
        }
    }
    let mut order: Vec<_> = (0..documents.len()).filter(|&i| fused[i] > 0.0).collect();
    order.sort_by(|&a, &b| fused[b].total_cmp(&fused[a]).then(a.cmp(&b)));
    order.into_iter().map(|i| documents[i]).collect()
}

fn render(index: &Index, request: &Request, vector: &[f32]) -> String {
    let mut results: Vec<(&str, &Chunk)> = Vec::new();
    for (path, chunk) in ranked(index, &request.query, vector) {
        if results.iter().any(|(old_path, old)| {
            *old_path == path && old.start <= chunk.end && chunk.start <= old.end
        }) {
            continue;
        }
        results.push((path, chunk));
        if results.len() == request.limit {
            break;
        }
    }
    if results.is_empty() {
        return NO_MATCHES.to_owned();
    }
    results
        .into_iter()
        .map(|(path, chunk)| {
            let mut text = format!("{path}:{}-{}", chunk.start, chunk.end);
            for (i, line) in chunk.text.lines().take(request.lines).enumerate() {
                text.push_str(&format!("\n{}|{line}", chunk.start + i));
            }
            if request.lines > 0 && chunk.text.lines().count() > request.lines {
                text.push_str("\n[snippet limited; use read for surrounding code]");
            }
            text
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(super) async fn search(cwd: &Path, args: &Value) -> Result<String> {
    let request = Request::parse(cwd, args)?;
    let directory = model_directory().await?;
    tokio::task::spawn_blocking(move || {
        let (model_key, model) = load_model(&directory)?;
        let cache = cache_root()?.join("indexes").join(format!(
            "{}.json",
            hash(format!("{}\n{}", request.root.display(), request.content).as_bytes())
        ));
        let mut index = std::fs::read(&cache)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Index>(&bytes).ok())
            .filter(|index| index.version == INDEX_VERSION && index.model == model_key)
            .unwrap_or_else(|| Index {
                version: INDEX_VERSION,
                model: model_key,
                files: BTreeMap::new(),
            });
        let (changed, mut warnings) =
            refresh(&mut index, &request, |text| model.encode_single(text))?;
        let mut text = render(&index, &request, &model.encode_single(&request.query));
        if changed {
            if let Err(error) = serde_json::to_vec(&index)
                .context("unable to serialize search index")
                .and_then(|bytes| atomic_write(&cache, &bytes))
            {
                warnings.push(format!("index cache not saved: {error:#}"));
            }
        }
        if !warnings.is_empty() {
            let count = warnings.len();
            warnings.truncate(10);
            text.push_str(&format!(
                "\n\n[index notes: {count}]\n{}",
                warnings.join("\n")
            ));
        }
        Ok(text)
    })
    .await
    .context("native code search worker failed")?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{enforce_path_scope, PathScope};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let mut nonce = [0u8; 8];
            getrandom::getrandom(&mut nonce).unwrap();
            let path = std::env::temp_dir().join(format!("oxide_code_search_{}", hash(&nonce)));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn write(&self, path: &str, text: &str) {
            let path = self.0.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        fn request(&self) -> Request {
            Request::parse(&self.0, &json!({"query":"authenticate users"})).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn validates_requests_and_applies_shared_path_boundaries() {
        let dir = Fixture::new();
        for args in [
            json!({}),
            json!({"query":" "}),
            json!({"query":"x","limit":0}),
            json!({"query":"x","limit":21}),
            json!({"query":"x","limit":-1}),
            json!({"query":"x","max_snippet_lines":201}),
            json!({"query":"x","content":"invalid"}),
            json!({"query":"x","path":1}),
            json!({"query":"x","path":"https://github.com/MinishLab/semble"}),
        ] {
            assert!(Request::parse(&dir.0, &args).is_err(), "{args}");
        }
        let request = Request::parse(
            &dir.0,
            &json!({"query":"--help $(touch absent)","max_snippet_lines":0}),
        )
        .unwrap();
        assert_eq!(request.query, "--help $(touch absent)");
        assert_eq!(request.lines, 0);
        assert!(enforce_path_scope(
            &dir.0,
            "code_search",
            &json!({"path":".."}),
            PathScope::CurrentProject
        )
        .is_err());
        assert!(
            enforce_path_scope(&dir.0, "code_search", &json!({}), PathScope::CurrentProject)
                .is_ok()
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.0.parent().unwrap(), dir.0.join("escape")).unwrap();
            assert!(enforce_path_scope(
                &dir.0,
                "code_search",
                &json!({"path":"escape"}),
                PathScope::CurrentProject
            )
            .is_err());
        }
    }

    #[test]
    fn syntax_chunks_keep_real_line_numbers_and_bound_large_functions() {
        for (extension, text, expected) in [
            (
                "rs",
                "// comment\nfn authenticate() {\n    verify();\n}\n\nfn other() {}\n",
                "fn authenticate() {\n    verify();\n}",
            ),
            (
                "py",
                "# comment\ndef authenticate():\n    verify()\n\ndef other():\n    pass\n",
                "def authenticate():\n    verify()",
            ),
            (
                "ts",
                "// comment\nfunction authenticate() {\n  verify();\n}\n",
                "function authenticate() {\n  verify();\n}",
            ),
        ] {
            let chunks = chunks(Path::new(&format!("auth.{extension}")), text);
            let found = chunks.iter().find(|chunk| chunk.text == expected).unwrap();
            assert_eq!(found.start, 2);
            assert_eq!(found.end, expected.lines().count() + 1);
        }
        let text = format!("fn large() {{\n{}}}\n", "    operation();\n".repeat(200));
        let parts = chunks(Path::new("large.rs"), &text);
        assert!(parts.len() > 3);
        assert!(parts
            .iter()
            .all(|chunk| chunk.text.len() <= CHUNK_BYTES && chunk.end - chunk.start < CHUNK_LINES));
        let original: Vec<_> = text.lines().collect();
        for chunk in &parts {
            assert_eq!(chunk.text, original[chunk.start - 1..chunk.end].join("\n"));
        }
        assert!(chunks(Path::new("huge.js"), &"x".repeat(CHUNK_BYTES + 1)).is_empty());
    }

    #[test]
    fn refresh_reuses_vectors_and_handles_changes_deletions_and_ignore_rules() {
        let dir = Fixture::new();
        dir.write("auth.rs", "fn authenticate() {}\n");
        dir.write(".gitignore", "ignored.rs\n");
        dir.write(".sembleignore", "nested/private.rs\n");
        for path in [
            "ignored.rs",
            "nested/private.rs",
            "target/build.rs",
            "node_modules/lib.js",
        ] {
            dir.write(path, "fn ignored() {}\n");
        }
        dir.write("nested/.gitignore", "more.rs\n");
        dir.write("nested/more.rs", "fn ignored() {}\n");
        dir.write("README.md", "Authentication guide\n");
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.0.join("auth.rs"), dir.0.join("alias.rs")).unwrap();
        let request = dir.request();
        let mut index = Index::default();
        let mut calls = 0;
        let (changed, warnings) = refresh(&mut index, &request, |_| {
            calls += 1;
            vec![1.0, 0.0]
        })
        .unwrap();
        assert!(changed);
        assert!(warnings.is_empty());
        assert_eq!(index.files.keys().cloned().collect::<Vec<_>>(), ["auth.rs"]);
        assert_eq!(calls, 1);
        assert!(
            !refresh(&mut index, &request, |_| panic!(
                "unchanged file re-embedded"
            ))
            .unwrap()
            .0
        );
        // Same-sized edits are detected even when timestamps do not distinguish them.
        let timestamp = std::fs::metadata(dir.0.join("auth.rs"))
            .unwrap()
            .modified()
            .unwrap();
        dir.write("auth.rs", "fn unauthorized() {}\n");
        std::fs::File::options()
            .write(true)
            .open(dir.0.join("auth.rs"))
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(timestamp))
            .unwrap();
        assert!(refresh(&mut index, &request, |_| vec![0.0, 1.0]).unwrap().0);
        assert_eq!(index.files["auth.rs"].chunks[0].vector, [0.0, 1.0]);
        dir.write("new.rs", "fn added() {}\n");
        std::fs::remove_file(dir.0.join("auth.rs")).unwrap();
        refresh(&mut index, &request, |_| vec![1.0, 0.0]).unwrap();
        assert_eq!(index.files.keys().cloned().collect::<Vec<_>>(), ["new.rs"]);
        dir.write(".gitignore", "ignored.rs\nnew.rs\n");
        refresh(&mut index, &request, |_| panic!("ignored file embedded")).unwrap();
        assert!(index.files.is_empty());
    }

    fn sample_index() -> Index {
        let mut index = Index::default();
        for (path, text, vector) in [
            ("auth.rs", "fn validate_credentials() {}", vec![1.0, 0.0]),
            (
                "math.rs",
                "fn sum(a: i32, b: i32) { a + b }",
                vec![0.0, 1.0],
            ),
        ] {
            index.files.insert(
                path.to_owned(),
                IndexedFile {
                    digest: String::new(),
                    chunks: vec![Chunk {
                        start: 10,
                        end: 10,
                        text: text.to_owned(),
                        vector,
                    }],
                },
            );
        }
        index
    }

    #[test]
    fn hybrid_search_retrieves_semantics_and_rare_identifiers() {
        let index = sample_index();
        assert_eq!(
            ranked(&index, "authenticate users", &[1.0, 0.0])[0].0,
            "auth.rs"
        );
        assert_eq!(ranked(&index, "sum", &[0.0, 0.0])[0].0, "math.rs");
        assert!(ranked(&index, "absent", &[0.0, 0.0]).is_empty());
        assert_eq!(
            terms("parseConfig foo_bar"),
            ["parse", "config", "foo", "bar"]
        );
        let dir = Fixture::new();
        let mut request = dir.request();
        request.lines = 0;
        assert_eq!(render(&index, &request, &[1.0, 0.0]), "auth.rs:10-10");
        request.lines = 80;
        assert!(render(&index, &request, &[1.0, 0.0]).contains("10|fn validate_credentials() {}"));
        let bytes = serde_json::to_vec(&index).unwrap();
        let loaded = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            render(&index, &request, &[1.0, 0.0]),
            render(&loaded, &request, &[1.0, 0.0])
        );
    }

    #[tokio::test]
    #[ignore = "downloads the real embedding model on first use; run explicitly with network access"]
    async fn native_model_smoke() {
        let directory = model_directory().await.unwrap();
        let (_, model) = load_model(&directory).unwrap();
        let dir = Fixture::new();
        dir.write(
            "auth.rs",
            "pub fn validate_credentials(password: &str) -> bool { password == \"secret\" }\n",
        );
        dir.write("math.rs", "pub fn sum(a: i32, b: i32) -> i32 { a + b }\n");
        let request = dir.request();
        let mut index = Index::default();
        refresh(&mut index, &request, |text| model.encode_single(text)).unwrap();
        let result = render(&index, &request, &model.encode_single(&request.query));
        assert!(result.starts_with("auth.rs:"), "{result}");
        eprintln!("native semantic retrieval: {result}");
        let call = crate::llm::ToolCall {
            id: "native-search".into(),
            kind: "function".into(),
            signature: None,
            function: crate::llm::FunctionCall {
                name: "code_search".into(),
                arguments: json!({"query": request.query}).to_string(),
            },
        };
        let start = std::time::Instant::now();
        let first = crate::tools::execute_scoped(
            &call,
            &dir.0,
            &crate::mcp::McpRegistry::default(),
            &crate::tools::Progress::default(),
            true,
        )
        .await;
        assert!(!first.is_error, "{}", first.text);
        assert!(first.text.starts_with("auth.rs:"), "{}", first.text);
        let cold = start.elapsed();
        let start = std::time::Instant::now();
        let second = crate::tools::execute_scoped(
            &call,
            &dir.0,
            &crate::mcp::McpRegistry::default(),
            &crate::tools::Progress::default(),
            true,
        )
        .await;
        assert!(!second.is_error, "{}", second.text);
        assert_eq!(first.text, second.text);
        eprintln!("registered tool: cold {cold:?}, warm {:?}", start.elapsed());
    }
}

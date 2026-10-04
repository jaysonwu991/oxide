// Hutch resolves a project's build scripts only from this file, and the exact
// Electrobun release named here is the devkit it projects into
// `.hutch/devkit/`, the Rust SDK this crate's manifest depends on.
export default {
	electrobun: { version: "2.0.2" },
	scripts: {
		dev: ["hutch", "electrobun", "dev", "--watch"],
		build: ["hutch", "electrobun", "build", "--env=stable"],
	},
};

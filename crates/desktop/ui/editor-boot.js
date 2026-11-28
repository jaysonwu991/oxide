// Monaco is loaded on first use so launch does not parse a language runtime the
// reader may never open. The loader and worker policy stay local to the app.
(() => {
  let loading = null;
  globalThis.oxideLoadMonaco = () => {
    if (loading) return loading;
    loading = new Promise((resolve, reject) => {
      const loader = globalThis.require;
      if (typeof loader?.config !== "function") {
        reject(new Error("Monaco's loader did not start."));
        return;
      }
      const vs = new URL("vendor/monaco/vs", document.baseURI).toString().replace(/\/$/, "");
      loader.config({ paths: { vs } });
      loader(
        ["vs/editor/editor.main"],
        () => {
          if (globalThis.monaco?.editor) resolve(globalThis.monaco);
          else reject(new Error("Monaco loaded without an editor."));
        },
        reject,
      );
    });
    return loading;
  };
})();

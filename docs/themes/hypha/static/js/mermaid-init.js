(function () {
  if (!window.mermaid) {
    console.error("Mermaid.js not loaded; cannot initialize diagrams");
    return;
  }

  function bootstrapMermaid() {
    const codeBlocks = Array.from(document.querySelectorAll("pre > code.language-mermaid"));
    codeBlocks.forEach((codeBlock) => {
      const pre = codeBlock.parentElement;
      const container = document.createElement("div");
      container.className = "mermaid";
      container.textContent = codeBlock.textContent;
      pre.replaceWith(container);
    });

    if (codeBlocks.length > 0) {
      try {
        window.mermaid.initialize({ startOnLoad: false });
        window.mermaid.run({ querySelector: ".mermaid" });
      } catch (error) {
        console.error("Failed to initialize Mermaid diagrams", error);
      }
    }
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", bootstrapMermaid);
  } else {
    bootstrapMermaid();
  }
})();

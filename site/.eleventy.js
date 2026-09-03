const markdownItAnchor = require("markdown-it-anchor");
const markdownItAttrs = require("markdown-it-attrs");
const markdownItContainer = require("markdown-it-container");
const syntaxHighlight = require("@11ty/eleventy-plugin-syntaxhighlight");

// Named blocks the markdown sources can open with `::: name` fences.
// Each entry: [container name, output element tag].
// `concept-section` is NOT here: it is auto-emitted around each H2 region
// by the `wrap_concept_sections` core rule below.
const CONTAINERS = [
  // Doc-page structural blocks
  ["concept-prose", "div"],
  ["concept-tree", "div"],
  ["concept-exchange", "div"],
  ["concept-ops", "div"],
  ["concept-op", "div"],
  ["concept-highlight", "div"],
  ["hierarchy-stack", "div"],
  ["hierarchy-level", "div"],
  ["lineage-stack", "div"],
  ["lineage-item", "div"],
  ["consequence-grid", "div"],
  ["consequence-card", "div"],
  ["pattern-principles", "div"],
  ["pattern-principle", "div"],
  // Landing-page blocks
  ["hero", "section"],
  ["philosophy", "section"],
  ["philosophy-lead", "div"],
  ["philosophy-grid", "div"],
  ["features", "section"],
  ["feature-grid", "div"],
  ["feature-card", "div"],
  ["install-section", "section"],
  ["hero-actions", "div"],
  // Implementation-page blocks
  ["impl-install", "div"],
  ["impl-links", "div"],
  ["impl-table", "div"],
  // Playground
  ["playground-section", "section"],
];

// Wrap each H2 region (h2 + content until next h2 or EOF) in
// `<section class="concept-section">`. Mirrors the manual wrapping
// the source .njk pages used. Skips pages that opt out via frontmatter,
// but markdown pages with no H2 are left untouched.
function wrapConceptSections(state) {
  const tokens = state.tokens;
  const out = [];
  let inSection = false;

  const mkOpen = () => {
    const t = new state.Token("html_block", "", 0);
    t.content = '<section class="concept-section">\n';
    return t;
  };
  const mkClose = () => {
    const t = new state.Token("html_block", "", 0);
    t.content = "</section>\n";
    return t;
  };

  for (const tok of tokens) {
    const isH2 = tok.type === "heading_open" && tok.tag === "h2";
    const isNav =
      tok.type === "html_block" && /^\s*<nav\b/i.test(tok.content);

    if ((isH2 || isNav) && inSection) {
      out.push(mkClose());
      inSection = false;
    }
    if (isH2) {
      out.push(mkOpen());
      inSection = true;
    }
    out.push(tok);
  }
  if (inSection) out.push(mkClose());
  state.tokens = out;
}

module.exports = function (eleventyConfig) {
  eleventyConfig.addPlugin(syntaxHighlight);

  eleventyConfig.addPassthroughCopy("src/css");
  eleventyConfig.addPassthroughCopy("src/js");
  eleventyConfig.addPassthroughCopy("src/wasm/pkg");

  eleventyConfig.amendLibrary("md", (mdLib) => {
    mdLib.use(markdownItAnchor, {
      permalink: markdownItAnchor.permalink.ariaHidden({
        placement: "after",
        symbol: "#",
      }),
      level: [2, 3, 4],
    });

    mdLib.use(markdownItAttrs);

    for (const [name, tag] of CONTAINERS) {
      mdLib.use(markdownItContainer, name, {
        render(tokens, idx) {
          return tokens[idx].nesting === 1
            ? `<${tag} class="${name}">\n`
            : `</${tag}>\n`;
        },
      });
    }

    mdLib.core.ruler.push("wrap_concept_sections", wrapConceptSections);
  });

  return {
    dir: {
      input: "src",
      output: "_site",
      includes: "_includes",
      data: "_data",
    },
  };
};

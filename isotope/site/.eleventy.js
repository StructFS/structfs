const markdownItAnchor = require("markdown-it-anchor");
const syntaxHighlight = require("@11ty/eleventy-plugin-syntaxhighlight");

module.exports = function (eleventyConfig) {
  // The spec chapters under src/spec/ are copied in by build.sh and
  // gitignored; eleventy must not skip them.
  eleventyConfig.setUseGitIgnore(false);

  // Syntax highlighting (build-time, no client JS)
  eleventyConfig.addPlugin(syntaxHighlight);

  // Pass through static assets.
  eleventyConfig.addPassthroughCopy("src/css");

  // Markdown: heading anchors, plus a link rewrite so the spec's
  // chapter-to-chapter links (`01-blocks.md`, `10-wasi-tower.md#bindings`)
  // resolve to site URLs. Other relative links point at the GitHub tree.
  eleventyConfig.amendLibrary("md", (mdLib) => {
    mdLib.use(markdownItAnchor, {
      permalink: markdownItAnchor.permalink.ariaHidden({
        placement: "after",
        symbol: "#",
      }),
      level: [2, 3, 4],
    });

    const defaultRender =
      mdLib.renderer.rules.link_open ||
      ((tokens, idx, options, env, self) =>
        self.renderToken(tokens, idx, options));

    mdLib.renderer.rules.link_open = (tokens, idx, options, env, self) => {
      const token = tokens[idx];
      const href = token.attrGet("href");
      if (href) {
        const chapter = href.match(/^(?:\.\/)?(\d{2})-([a-z0-9-]+)\.md(#.*)?$/);
        if (chapter) {
          token.attrSet("href", `/spec/${chapter[2]}/${chapter[3] || ""}`);
        } else if (/^(?:\.\.?\/)/.test(href)) {
          // Anything else relative (rationale/, examples/, source files)
          // lives in the repo, not on the site.
          token.attrSet(
            "href",
            "https://github.com/StructFS/structfs/blob/main/isotope/spec/" +
              href,
          );
        }
      }
      return defaultRender(tokens, idx, options, env, self);
    };
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

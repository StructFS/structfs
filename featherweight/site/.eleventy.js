const markdownItAnchor = require("markdown-it-anchor");
const syntaxHighlight = require("@11ty/eleventy-plugin-syntaxhighlight");

module.exports = function (eleventyConfig) {
  // src/demo/ is copied in by build.sh and gitignored; eleventy must
  // not skip it.
  eleventyConfig.setUseGitIgnore(false);

  // Syntax highlighting (build-time, no client JS)
  eleventyConfig.addPlugin(syntaxHighlight);

  // Pass through static assets. src/demo/ holds the browser-host
  // modules and kv.wasm, copied in by build.sh.
  eleventyConfig.addPassthroughCopy("src/css");
  eleventyConfig.addPassthroughCopy("src/demo");
  // Cloudflare Pages headers: COOP/COEP for SharedArrayBuffer (the
  // resident demo). Site-wide — every asset here is same-origin.
  eleventyConfig.addPassthroughCopy({ "src/_headers": "_headers" });

  // Markdown: heading anchors.
  eleventyConfig.amendLibrary("md", (mdLib) => {
    mdLib.use(markdownItAnchor, {
      permalink: markdownItAnchor.permalink.ariaHidden({
        placement: "after",
        symbol: "#",
      }),
      level: [2, 3, 4],
    });
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

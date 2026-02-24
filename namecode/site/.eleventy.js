const markdownItAnchor = require("markdown-it-anchor");

module.exports = function (eleventyConfig) {
  // Pass through static assets
  eleventyConfig.addPassthroughCopy("src/css");
  eleventyConfig.addPassthroughCopy("src/js");
  eleventyConfig.addPassthroughCopy("src/wasm/pkg");

  // Configure markdown with heading anchors
  eleventyConfig.amendLibrary("md", (mdLib) => {
    mdLib.use(markdownItAnchor, {
      permalink: markdownItAnchor.permalink.headerLink(),
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

// Directory data for the spec chapters copied in by build.sh from
// ../../spec. Titles come from each chapter's own H1; URLs drop the
// numeric prefix ("03-namespaces.md" -> /spec/namespaces/).
const fs = require("fs");

module.exports = {
  layout: "spec.njk",
  tags: ["spec"],
  // Markdown only — no Liquid/Nunjucks preprocessing, so spec prose can
  // safely contain {{ }} or {% %} without breaking the build.
  templateEngineOverride: "md",
  eleventyComputed: {
    title: (data) => {
      const source = fs.readFileSync(data.page.inputPath, "utf8");
      const h1 = source.match(/^#\s+(.+)$/m);
      return h1 ? h1[1].replace(/^Isotope:\s*/, "") : data.page.fileSlug;
    },
    chapterNumber: (data) => data.page.fileSlug.slice(0, 2),
    permalink: (data) =>
      `/spec/${data.page.fileSlug.replace(/^\d+-/, "")}/`,
  },
};

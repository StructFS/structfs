// This site's identity. The canonical URL comes from the repo-root
// /sites.json so every site's domain is defined in one place.
const sites = require("../../../../sites.json");

module.exports = {
  title: "featherweight",
  url: sites.featherweight,
  description:
    "A single-node reference implementation of the Isotope specification, in Rust over StructFS.",
};

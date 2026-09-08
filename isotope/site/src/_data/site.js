// This site's identity. The canonical URL comes from the repo-root
// /sites.json so every site's domain is defined in one place.
const sites = require("../../../../sites.json");

module.exports = {
  title: "Isotope",
  url: sites.isotope,
  description:
    "An open specification for a virtual operating system where every system interaction is a read or write on a path.",
};

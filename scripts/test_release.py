"""Regression tests for release selection and fail-closed registry checks."""
import unittest
from unittest.mock import patch
import urllib.error
import release


def package(name, publish=None, dependencies=()):
    return dict(id=name, name=name, version="0.2.0", publish=publish,
                dependencies=list(dependencies))


def metadata(*packages):
    return dict(workspace_members=[p["id"] for p in packages], packages=list(packages))


class ReleaseTests(unittest.TestCase):
    def test_publication_policy(self):
        data = metadata(package("default"), package("private", []),
                        package("explicit", ["crates-io"]), package("other", ["internal"]))
        self.assertEqual([p["name"] for p in release.plan(data)], ["default", "explicit"])

    def test_order_including_dev_dependencies(self):
        a = package("a", dependencies=[dict(name="z", path="z", kind=None)])
        z = package("z")
        self.assertEqual([p["name"] for p in release.plan(metadata(a, z))], ["z", "a"])
        z["dependencies"] = [dict(name="a", path="a", kind="dev")]
        with self.assertRaisesRegex(ValueError, "cycle"):
            release.plan(metadata(a, z))

    def test_filters_and_unknown_names(self):
        data = metadata(package("a"), package("b"))
        self.assertEqual([p["name"] for p in release.plan(data, only=["b"])], ["b"])
        with self.assertRaisesRegex(ValueError, "unknown"):
            release.plan(data, only=["typo"])
        with self.assertRaisesRegex(ValueError, "no crates"):
            release.plan(data, exclude=["a", "b"])

    def test_private_dependency_rejected(self):
        data = metadata(package("a", dependencies=[dict(name="b", path="b", kind=None)]),
                        package("b", []))
        with self.assertRaisesRegex(ValueError, "nonpublishable"):
            release.plan(data)

    def test_only_404_means_missing(self):
        for status in [403, 429, 500]:
            with self.subTest(status=status), patch("release.urllib.request.urlopen",
                    side_effect=urllib.error.HTTPError("url", status, "failure", {}, None)):
                with self.assertRaisesRegex(RuntimeError, "registry lookup failed"):
                    release.registry_versions("structfs")
        with patch("release.urllib.request.urlopen",
                   side_effect=urllib.error.HTTPError("url", 404, "missing", {}, None)):
            self.assertEqual(release.registry_versions("new-crate"), {})

    def test_omitted_dependencies_must_already_be_published(self):
        a = package("a", dependencies=[dict(name="b", path="b", req="^0.2.0")])
        data = metadata(a, package("b"))
        for entry in [{}, {"0.2.0": {"yanked": True}}]:
            with self.assertRaisesRegex(ValueError, "omitted dependency"):
                release.check_external_dependencies([a], data, lambda _: entry)
        release.check_external_dependencies([a], data,
                                            lambda _: {"0.2.0": {"yanked": False}})

    def test_dry_run_never_publishes_or_tags(self):
        data = metadata(package("example"))
        def command(*args, **kwargs):
            if args[:2] == ("cargo", "metadata"):
                return release.json.dumps(data)
            if args[:2] == ("git", "rev-parse"):
                return "abc"
            return ""
        with patch("sys.argv", ["release.py", "--dry-run"]), \
                patch("release.run", side_effect=command) as run, \
                patch("release.registry_versions", return_value={}), \
                patch("release.subprocess.run") as git:
            git.return_value.returncode = 1
            release.main()
            self.assertTrue(any("check-release.sh" in str(call.args) for call in run.call_args_list))
            self.assertFalse(any(call.args[:2] in [("cargo", "publish"), ("git", "tag")]
                                 for call in run.call_args_list))

    def test_registry_polling(self):
        with patch("release.registry_versions", side_effect=[{}, {"0.2.0": {"yanked": False}}]), \
                patch("release.time.sleep"):
            release.wait_for_version("example", "0.2.0", 30)
        with patch("release.registry_versions", return_value={"0.2.0": {"yanked": True}}):
            with self.assertRaisesRegex(RuntimeError, "yanked"):
                release.wait_for_version("example", "0.2.0", 30)


if __name__ == "__main__":
    unittest.main()

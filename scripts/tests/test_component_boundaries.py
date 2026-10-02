"""Keep production dependencies within the boundaries in ADR 0004."""
import pathlib
import tomllib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]


def dependency_graph():
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())
    graph = {}
    for member in workspace["workspace"]["members"]:
        manifest = tomllib.loads((ROOT / member / "Cargo.toml").read_text())
        dependencies = set(manifest.get("dependencies", {}))
        for target in manifest.get("target", {}).values():
            dependencies.update(target.get("dependencies", {}))
        graph[manifest["package"]["name"]] = dependencies
    return graph


def reachable(graph, package):
    seen = set()
    pending = list(graph.get(package, ()))
    while pending:
        dependency = pending.pop()
        if dependency not in seen:
            seen.add(dependency)
            pending.extend(graph.get(dependency, ()))
    return seen


class ComponentBoundaries(unittest.TestCase):
    def test_shell_does_not_depend_on_core(self):
        self.assertNotIn("ferese-core", reachable(dependency_graph(), "ferese-shell"))

    def test_theme_model_has_no_configuration_or_runtime_dependencies(self):
        dependencies = reachable(dependency_graph(), "ferese-theme-model")
        self.assertTrue(dependencies.isdisjoint({
            "ferese-config", "ferese-ipc", "ferese-theme", "ferese-core",
            "libcosmic", "wayland-client", "tokio", "kdl", "jiff",
        }), dependencies)

    def test_ipc_does_not_depend_on_configuration(self):
        self.assertNotIn("ferese-config", reachable(dependency_graph(), "ferese-ipc"))

    def test_transitive_dependencies_are_checked(self):
        self.assertIn("core", reachable({"shell": {"helper"}, "helper": {"core"}}, "shell"))


if __name__ == "__main__":
    unittest.main()

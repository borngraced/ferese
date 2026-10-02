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

    def test_transitive_dependencies_are_checked(self):
        self.assertIn("core", reachable({"shell": {"helper"}, "helper": {"core"}}, "shell"))


if __name__ == "__main__":
    unittest.main()

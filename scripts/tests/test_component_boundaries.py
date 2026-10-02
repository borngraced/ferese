"""Keep production dependencies within the boundaries in ADR 0004."""
import pathlib
import tomllib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]


def production_dependencies(manifest, workspace_dependencies):
    dependencies = set()
    for section in [manifest, *manifest.get("target", {}).values()]:
        for kind in ("dependencies", "build-dependencies"):
            for alias, specification in section.get(kind, {}).items():
                if isinstance(specification, dict) and specification.get("workspace"):
                    specification = workspace_dependencies[alias]
                name = specification.get("package", alias) if isinstance(specification, dict) else alias
                dependencies.add(name)
    return dependencies


def dependency_graph():
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]
    graph = {}
    for member in workspace["members"]:
        manifest = tomllib.loads((ROOT / member / "Cargo.toml").read_text())
        graph[manifest["package"]["name"]] = production_dependencies(manifest, workspace.get("dependencies", {}))
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

    def test_rendering_does_not_load_configuration_or_own_clients(self):
        dependencies = reachable(dependency_graph(), "ferese-theme")
        self.assertTrue(dependencies.isdisjoint({
            "ferese-config", "ferese-ipc", "ferese-theme-client", "ferese-protocols",
        }), dependencies)

    def test_client_integration_does_not_depend_on_rendering_or_core(self):
        dependencies = reachable(dependency_graph(), "ferese-theme-client")
        self.assertTrue(dependencies.isdisjoint({"ferese-theme", "ferese-core"}), dependencies)

    def test_aliases_build_dependencies_and_target_dependencies_are_checked(self):
        manifest = {
            "dependencies": {"alias": {"workspace": True}},
            "build-dependencies": {"builder": {"package": "ferese-config"}},
            "target": {"cfg(unix)": {"dependencies": {"client": {"package": "ferese-theme-client"}}}},
            "dev-dependencies": {"ferese-theme": {}},
        }
        self.assertEqual(production_dependencies(manifest, {"alias": {"package": "ferese-core"}}),
                         {"ferese-core", "ferese-config", "ferese-theme-client"})

    def test_transitive_dependencies_are_checked(self):
        self.assertIn("core", reachable({"shell": {"helper"}, "helper": {"core"}}, "shell"))


if __name__ == "__main__":
    unittest.main()

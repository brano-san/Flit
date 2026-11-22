use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

const LAYERS: &[&[&str]] = &[
    &["flit-core", "flit-gen", "xtask"],
    &["flit-proto", "flit-sign"],
    &["flit-search"],
    &["flit-platform"],
    &["flit-archive"],
    &["flit-platform-linux", "flit-platform-windows"],
    &["flit-platform-linux-gui"],
    &["flit", "flit-indexd"],
];
const PORTABLE: &[&str] = &["flit-core", "flit-proto", "flit-search", "flit-platform"];

#[derive(Deserialize)]
pub(crate) struct Metadata {
    packages: Vec<Package>,
    workspace_members: HashSet<String>,
    resolve: Resolve,
}

#[derive(Deserialize)]
pub(crate) struct Package {
    id: String,
    name: String,
    pub(crate) manifest_path: PathBuf,
    dependencies: Vec<DeclaredDep>,
}

#[derive(Deserialize)]
struct DeclaredDep {
    name: String,
    path: Option<PathBuf>,
}

#[derive(Deserialize)]
struct Resolve {
    nodes: Vec<Node>,
}

#[derive(Deserialize)]
struct Node {
    id: String,
    deps: Vec<ResolvedDep>,
}

#[derive(Deserialize)]
struct ResolvedDep {
    pkg: String,
    dep_kinds: Vec<DepKind>,
}

#[derive(Deserialize)]
struct DepKind {
    kind: Option<String>,
}

fn layer(name: &str) -> Option<usize> {
    LAYERS.iter().position(|names| names.contains(&name))
}

pub(crate) fn check<'a>(metadata: &'a Metadata, problems: &mut Vec<String>) -> Vec<&'a Package> {
    let members: Vec<_> = metadata.packages.iter().filter(|p| metadata.workspace_members.contains(&p.id)).collect();
    for name in LAYERS.iter().copied().flatten() {
        if !members.iter().any(|p| p.name == *name) {
            problems.push(format!("Stale layer entry: {name}"));
        }
    }
    for member in &members {
        let Some(own_layer) = layer(&member.name) else {
            problems.push(format!("Unregistered workspace crate: {}", member.name));
            continue;
        };
        for dep in &member.dependencies {
            let Some(path) = &dep.path else { continue };
            let registered = members.iter().any(|p| p.name == dep.name && p.manifest_path.parent() == Some(path));
            match layer(&dep.name) {
                Some(dep_layer) if registered => {
                    if dep_layer > own_layer {
                        problems.push(format!("{} (layer {own_layer}) depends on {} (layer {dep_layer})", member.name, dep.name));
                    }
                }
                _ => problems.push(format!("{} depends on unregistered local crate {} at {}", member.name, dep.name, path.display())),
            }
        }
    }
    check_windows(metadata, &members, problems);
    members
}

fn check_windows(metadata: &Metadata, members: &[&Package], problems: &mut Vec<String>) {
    let names: HashMap<_, _> = metadata.packages.iter().map(|p| (p.id.as_str(), p.name.as_str())).collect();
    let nodes: HashMap<_, _> = metadata.resolve.nodes.iter().map(|n| (n.id.as_str(), &n.deps)).collect();
    for member in members.iter().filter(|p| PORTABLE.contains(&p.name.as_str())) {
        let mut pending = vec![member.id.as_str()];
        let mut seen = HashSet::new();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            if let Some(name) = names.get(id).filter(|name| name.starts_with("windows")) {
                problems.push(format!("{} transitively depends on {name}", member.name));
            }
            if let Some(deps) = nodes.get(id) {
                pending.extend(deps.iter().filter(|d| d.dep_kinds.iter().any(|k| k.kind.as_deref() != Some("dev"))).map(|d| d.pkg.as_str()));
            }
        }
    }
}

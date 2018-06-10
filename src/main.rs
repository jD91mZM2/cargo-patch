#[macro_use] extern crate clap;
extern crate cargo;
extern crate toml;

use cargo::{
    CargoResult,
    core::{Package, PackageId, Workspace},
    ops,
    util::{config::Config, important_paths}
};
use clap::{App as Clap, Arg, SubCommand};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env,
    fmt,
    fs,
    io,
    path::{Path, PathBuf}
};

enum PackagePath<'a> {
    Git(&'a str),
    Path(PathBuf)
}
struct StackEntry<'a, I>
    where I: Iterator<Item = &'a PackageId>
{
    package: &'a Package,
    dependencies: I,
    updated: Option<HashMap<String, PackagePath<'a>>>
}
impl<'a, I> fmt::Debug for StackEntry<'a, I>
    where I: Iterator<Item = &'a PackageId>
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.package)
    }
}

fn main() -> CargoResult<()> {
    let matches = Clap::new(crate_name!())
        .about(crate_description!())
        .author(crate_authors!())
        .version(crate_version!())
        .subcommand(SubCommand::with_name("patch")
            .about(crate_description!())
            .author(crate_authors!())
            .version(crate_version!())
            .arg(Arg::with_name("replace")
                .long("replace")
                .takes_value(true)
                .multiple(true)))
        .get_matches();

    if matches.subcommand_name() != Some("patch") {
        eprintln!("Don't run the binary directly, run it with cargo:");
        eprintln!("cargo patch");
        return Ok(());
    }
    let matches = matches.subcommand_matches("patch").expect("Subcommand is patch but no matches for patch");

    let mut replace = HashMap::with_capacity(16);

    if let Some(values) = matches.values_of("replace") {
        for value in values {
            let mut parts = value.splitn(2, '=');
            match (parts.next(), parts.next()) {
                (Some(name), Some(url)) => { replace.insert(name, url); },
                _ => {
                    eprintln!("Incorrect syntax for replace.");
                    eprintln!("Use name=url");
                    return Ok(());
                }
            }
        }
    }

    let cwd = env::current_dir()?;
    let manifest = important_paths::find_root_manifest_for_wd(&cwd)?;
    let config = Config::default()?;
    let workspace = Workspace::new(&manifest, &config)?;
    let package = workspace.current()?;

    let (packages, resolve) = ops::resolve_ws(&workspace)?;

    let basedir = cwd.join("cargo-patch");
    if !basedir.exists() {
        fs::create_dir(&basedir)?;
    } else if !basedir.is_dir() {
        eprintln!("File \"cargo-patch\" exists but is not a folder.");
        eprintln!("But I need this directory...");
        return Ok(());
    }

    let mut cache = HashSet::with_capacity(64);
    let mut stack = Vec::with_capacity(64);

    stack.push(StackEntry {
        package: package,
        dependencies: resolve.deps(package.package_id()),
        updated: None
    });

    loop {
        let mut to_add = None;
        {
            let entry = match stack.last_mut() {
                Some(entry) => entry,
                None => break
            };

            if let Some(id) = entry.dependencies.next() {
                let package = packages.get(&id)?;

                if let Some(url) = replace.get(&*package.name()) {
                    entry.updated.get_or_insert_with(|| HashMap::with_capacity(4))
                        .insert(package.name().to_string(), PackagePath::Git(url));
                    continue;
                } else if cache.contains(&entry.package.package_id()) {
                    let name = entry.package.name().to_string();

                    let path = basedir.join(&name);
                    entry.updated.get_or_insert_with(|| HashMap::with_capacity(4))
                        .insert(name.clone(), PackagePath::Path(path));
                } else {
                    // Can't push while .last() is borrowed
                    to_add = Some(package);
                }
            }
        }

        let mut name = None;

        if let Some(package) = to_add {
            let id = package.package_id();
            if stack.iter().all(|entry| entry.package.package_id() != id) {
                stack.push(StackEntry {
                    package: package,
                    dependencies: resolve.deps(&id).into_iter(),
                    updated: None
                });
            } else {
                eprintln!("Stuck in dependency loop!");
                eprintln!("Package wants {}, but this appears previously in the stack!", package);
                return Ok(());
            }
        } else if let Some(entry) = stack.pop() {
            let package = entry.package;
            if cache.contains(&entry.package.package_id()) {
                name = Some(package.name());
            } else if let Some(ref replaces) = entry.updated {
                let _dest;
                let manifest = if !stack.is_empty() {
                    let path = package.manifest_path().parent().expect("Manifest path didn't have parent");
                    let dest = basedir.join(&*package.name());
                    if dest.exists() {
                        println!("Skipping {}", package.name());
                    } else {
                        println!("Copying {}...", package.name());
                        copy(&path, &dest)?;
                    }
                    name = Some(package.name());
                    _dest = dest.join("Cargo.toml");
                    &_dest
                } else {
                    &manifest
                };
                let contents = fs::read_to_string(manifest)?;
                let mut parsed: toml::Value = toml::from_str(&contents)?;
                for table in &["dependencies", "dev-dependencies", "build-dependencies"] {
                    if let Some(deps) = parsed.get_mut(table) {
                        for (key, value) in replaces {
                            if let Some(dep) = deps.get_mut(key) {
                                fn change_path(map: &mut BTreeMap<String, toml::Value>, value: &PackagePath) {
                                    for key in &["version", "path", "git"] {
                                        map.remove(*key);
                                    }
                                    match value {
                                        PackagePath::Path(path) => {
                                            map.insert(String::from("path"), toml::Value::String(
                                                path.to_string_lossy().into_owned()
                                            ));
                                        }
                                        PackagePath::Git(url) => {
                                            map.insert(String::from("git"), toml::Value::String(url.to_string()));
                                        }
                                    }
                                }
                                match dep {
                                    toml::Value::Table(inner) => change_path(inner, value),
                                    toml::Value::String(_) => {
                                        let mut map = BTreeMap::new();
                                        change_path(&mut map, value);
                                        *dep = toml::Value::Table(map);
                                    },
                                    _ => {
                                        eprintln!("Invalid value in Cargo.toml");
                                        eprintln!("Dependency {:?} is not a string nor a table", key);
                                        return Ok(());
                                    }
                                }
                            }
                        }
                    }
                }
                fs::write(manifest, toml::to_string_pretty(&parsed)?)?;

                cache.insert(package.package_id());
            }
        }
        if let Some(name) = name {
            if let Some(entry) = stack.last_mut() {
                let path = basedir.join(&*name);
                entry.updated.get_or_insert_with(|| HashMap::with_capacity(4))
                    .insert(name.to_string(), PackagePath::Path(path));
            }
        }
    }

    Ok(())
}
fn copy<P1, P2>(src: P1, dst: P2) -> io::Result<()>
    where P1: AsRef<Path>,
          P2: AsRef<Path>
{
    let src = src.as_ref();
    let dst = dst.as_ref();
    debug_assert!(!dst.exists());

    if src.is_dir() {
        fs::create_dir(dst)?;
        for entry in fs::read_dir(src)? {
            let path = entry?.path();
            copy(&path, dst.join(&path.strip_prefix(src).unwrap()))?;
        }
    } else {
        fs::copy(src, dst)?;
    }
    Ok(())
}

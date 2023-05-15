use cargo_scan::audit_chain::AuditChain;
use cargo_scan::auditing::audit::audit_policy;
use cargo_scan::auditing::info::Config as AuditConfig;
use cargo_scan::ident::CanonicalPath;
use cargo_scan::policy::PolicyFile;
use cargo_scan::util::load_cargo_toml;
use cargo_scan::{download_crate, scanner};

use anyhow::{anyhow, Context, Result};
use cargo::{core::Workspace, ops::generate_lockfile, util::config};
use cargo_lock::{Dependency, Lockfile, Package};
use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};
use petgraph::graph::{DiGraph, NodeIndex};
use petgraph::visit::DfsPostOrder;
use std::collections::{HashMap, HashSet};
use std::fs::{create_dir_all, remove_file};
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
struct Args {
    // TODO: Can probably use the default rust build location
    /// Path to download crates to for auditing
    #[clap(short = 'd', long = "crate-download-path", default_value = ".audit_crates")]
    crate_download_path: String,

    #[clap(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    Create(Create),
    Review(Review),
    Audit(Audit),
}

// TODO: Add an argument for the default policy type
#[derive(Clone, ClapArgs, Debug)]
struct Create {
    /// Path to crate
    crate_path: String,
    /// Path to manifest
    manifest_path: String,

    // TODO: Check to make sure it meets the format (clap supports this?)
    /// Default policy folder
    #[clap(short = 'p', long = "policy-path", default_value = ".audit_policies")]
    policy_path: String,

    #[clap(short = 'f', long, default_value_t = false)]
    force_overwrite: bool,
}

#[derive(Clone, ClapArgs, Debug)]
struct Review {
    /// Path to chain manifest
    manifest_path: String,
    /// Crate to review
    crate_name: String,
    /// What information to present in review
    #[clap(short = 't', long, default_value_t = ReviewType::PubFuns)]
    review_type: ReviewType,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
enum ReviewType {
    PubFuns,
    All,
}

impl std::fmt::Display for ReviewType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ReviewType::PubFuns => "pub-funs",
            ReviewType::All => "all",
        };
        write!(f, "{}", s)
    }
}

#[derive(Clone, ClapArgs, Debug)]
struct Audit {
    /// Path to manifest
    manifest_path: String,
    /// Crate to review
    crate_name: String,
}

// TODO: Different default policies
/// Creates a new default policy for the given package and returns the path to
/// the saved policy file
fn make_new_policy(
    chain: &AuditChain,
    package: &Package,
    root_name: &str,
    args: &Create,
    crate_download_path: &str,
) -> Result<PathBuf> {
    let policy_path = PathBuf::from(format!(
        "{}/{}-{}.policy",
        args.policy_path,
        package.name.as_str(),
        package.version
    ));

    // download the new policy
    let package_path = if format!("{}-{}", package.name, package.version) == root_name {
        // We are creating a policy for the root crate
        PathBuf::from(args.crate_path.clone())
    } else {
        // TODO: Handle the case where we have a crate source not from crates.io
        download_crate::download_crate(package, crate_download_path)?
    };

    // Try to create a new default policy
    if policy_path.is_dir() {
        return Err(anyhow!("Policy path is a directory"));
    }
    if policy_path.is_file() {
        if args.force_overwrite {
            remove_file(policy_path.clone())?;
        } else {
            return Err(anyhow!("Policy file already exists"));
        }
    }

    let sinks = collect_dependency_sinks(chain, &package.dependencies)?;
    let policy =
        PolicyFile::new_caller_checked_default_with_sinks(package_path.as_path(), sinks)?;
    policy.save_to_file(policy_path.clone())?;

    Ok(policy_path)
}

fn create_audit_chain_dirs(args: &Create, crate_download_path: &str) -> Result<()> {
    let mut manifest_path = PathBuf::from(&args.manifest_path);
    manifest_path.pop();
    create_dir_all(manifest_path)?;

    let crate_download_path = PathBuf::from(crate_download_path);
    create_dir_all(crate_download_path)?;

    let policy_path = PathBuf::from(&args.policy_path);
    create_dir_all(policy_path)?;

    Ok(())
}

fn make_dependency_graph(
    packages: &Vec<Package>,
    root_name: &str,
) -> (DiGraph<String, ()>, HashMap<NodeIndex, Package>, NodeIndex) {
    let mut graph = DiGraph::new();
    let mut node_map = HashMap::new();
    let mut package_map = HashMap::new();

    for p in packages {
        let p_string = format!("{}-{}", p.name.as_str(), p.version);
        if !node_map.contains_key(&p_string) {
            let next_node = graph.add_node(p_string.clone());
            node_map.insert(p_string.clone(), next_node);
        }
        // Clone to avoid multiple mutable borrow
        let p_idx = *node_map.get(&p_string).unwrap();
        package_map.insert(p_idx, p.clone());

        for dep in &p.dependencies {
            let dep_string = format!("{}-{}", dep.name.as_str(), dep.version);
            if !node_map.contains_key(&dep_string) {
                let next_node = graph.add_node(dep_string.clone());
                node_map.insert(dep_string.clone(), next_node);
            }
            let dep_idx = *node_map.get(&dep_string).unwrap();
            graph.add_edge(p_idx, dep_idx, ());
        }
    }

    let root_idx = *node_map.get(root_name).unwrap();
    (graph, package_map, root_idx)
}

fn collect_dependency_sinks(
    chain: &AuditChain,
    deps: &Vec<Dependency>,
) -> Result<HashSet<CanonicalPath>> {
    let mut sinks = HashSet::new();
    for dep in deps {
        let dep_string = format!("{}-{}", dep.name, dep.version);
        let policy = chain.read_policy(&dep_string).context(
            "couldnt read dependency policy file (maybe created it out of order)",
        )?;
        sinks.extend(policy.pub_caller_checked.iter().cloned());
    }

    Ok(sinks)
}

fn create_new_audit_chain(args: Create, crate_download_path: &str) -> Result<AuditChain> {
    println!("Creating audit chain");
    let mut chain = AuditChain::new(
        PathBuf::from(&args.manifest_path),
        PathBuf::from(&args.crate_path),
    );

    create_audit_chain_dirs(&args, crate_download_path)?;

    println!("Loading audit package lockfile");
    // If the lockfile doesn't exist, generate it
    let lockfile =
        if let Ok(l) = Lockfile::load(format!("{}/Cargo.lock", args.crate_path)) {
            l
        } else {
            println!("Lockfile missing: generating new lockfile");
            let config = config::Config::default()?;
            let workspace = Workspace::new(Path::new(&args.crate_path), &config)?;
            generate_lockfile(&workspace)?;
            Lockfile::load(format!("{}/Cargo.lock", args.crate_path))?
        };

    let crate_data = load_cargo_toml(&PathBuf::from(&args.crate_path))?;

    let root_name = format!("{}-{}", crate_data.name, crate_data.version);

    println!("Creating dependency graph");
    let (graph, package_map, root_node) =
        make_dependency_graph(&lockfile.packages, &root_name);
    let mut traverse = DfsPostOrder::new(&graph, root_node);
    while let Some(node) = traverse.next(&graph) {
        let package = package_map.get(&node).unwrap();
        println!("Making default policy for {} v{}", package.name, package.version);
        match make_new_policy(&chain, package, &root_name, &args, crate_download_path) {
            Ok(policy_path) => {
                chain.add_crate_policy(package, policy_path);
            }
            Err(e) => return Err(anyhow!("Audit chain creation failed: {}", e)),
        };
    }

    println!("Finished creating policy chain");
    Ok(chain)
}

fn review_policy(policy: &PolicyFile, review_type: ReviewType) {
    match review_type {
        // TODO: Plug in to existing policy review
        ReviewType::All => (),
        ReviewType::PubFuns => {
            println!("Public functions marked caller-checked:");
            for pub_fn in policy.pub_caller_checked.iter() {
                // TODO: Print more info
                println!("{}", pub_fn);
            }
        }
    }
}

fn runner(args: Args) -> Result<()> {
    match args.command {
        Command::Create(create) => {
            let chain = create_new_audit_chain(create)?;
            chain.save_to_file()?;
            Ok(())
        }
        Command::Audit(_audit) => Ok(()),
        Command::Review(review) => {
            println!("Reviewing crate: {}", review.crate_name);
            match AuditChain::read_audit_chain(PathBuf::from(&review.chain_path)) {
                Ok(Some(chain)) => {
                    let policies = chain.read_policy_no_version(&review.crate_name)?;
                    if policies.is_empty() {
                        println!("No policies matching the crate {}", &review.chain_path);
                        Ok(())
                    } else if policies.len() > 1 {
                        // TODO: Allow for reviewing more than one policy matching a crate
                        println!("More than one policy for crate {}", &review.chain_path);
                        Ok(())
                    } else {
                        let (full_crate_name, policy) = &policies[0];
                        println!("Reviewing policy for {}", full_crate_name);
                        review_policy(policy, review.review_type);
                        Ok(())
                    }
                }
                Ok(None) => Err(anyhow!(
                    "Couldn't find audit chain manifest at {}",
                    &review.manifest_path
                )),
                Err(e) => Err(e),
            }
        }
    }
}

fn main() {
    cargo_scan::util::init_logging();
    let args = Args::parse();

    match runner(args) {
        Ok(()) => (),
        Err(e) => println!("Error running command: {}", e),
    }
}

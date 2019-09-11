use cargo::core::{enable_nightly_features, Package, SourceId, Workspace as CargoWorkspace};
use cargo::sources::SourceConfigMap;
use cargo::util::{internal, Config};
use error::Result;
use rustwide::{
    cmd::{Command, SandboxBuilder},
    Crate, Toolchain, Workspace, WorkspaceBuilder,
};
use std::collections::HashSet;
use std::path::Path;
use utils::{parse_rustc_version, resolve_deps};
use Metadata;

// TODO: 1GB might not be enough
const SANDBOX_MEMORY_LIMIT: usize = 1024 * 1025 * 1024; // 1GB
const SANDBOX_NETWORKING: bool = false;

pub fn build_doc_rustwide(name: &str, version: &str, target: Option<&str>) -> Result<Package> {
    // TODO: Handle workspace path correctly
    let rustwide_workspace =
        WorkspaceBuilder::new(Path::new("/tmp/docs-builder"), "docsrs").init()?;

    // TODO: Instead of using just nightly, we can pin a version.
    //       Docs.rs can only use nightly (due to unstable docs.rs features in rustdoc)
    let toolchain = Toolchain::Dist {
        name: "nightly".into(),
    };
    toolchain.install(&rustwide_workspace)?;
    if let Some(target) = target {
        toolchain.add_target(&rustwide_workspace, target)?;
    }

    let krate = Crate::crates_io(name, version);
    krate.fetch(&rustwide_workspace)?;

    let sandbox = SandboxBuilder::new()
        .memory_limit(Some(SANDBOX_MEMORY_LIMIT))
        .enable_networking(SANDBOX_NETWORKING);

    let mut build_dir = rustwide_workspace.build_dir(&format!("{}-{}", name, version));
    let pkg = build_dir.build(&toolchain, &krate, sandbox, |build| {
        enable_nightly_features();
        let config = Config::default()?;
        let source_id = try!(SourceId::crates_io(&config));
        let source_cfg_map = try!(SourceConfigMap::new(&config));
        let manifest_path = build.host_source_dir().join("Cargo.toml");
        let ws = CargoWorkspace::new(&manifest_path, &config)?;
        let pkg = ws.load(&manifest_path)?;

        let metadata = Metadata::from_package(&pkg).map_err(|e| internal(e.to_string()))?;

        let mut rustdoc_flags: Vec<String> = vec![
            "-Z".to_string(),
            "unstable-options".to_string(),
            "--resource-suffix".to_string(),
            format!(
                "-{}",
                parse_rustc_version(rustc_version(&rustwide_workspace, &toolchain)?)?
            ),
            "--static-root-path".to_string(),
            "/".to_string(),
            "--disable-per-crate-search".to_string(),
        ];

        let source = try!(source_cfg_map.load(source_id, &HashSet::new()));
        let _lock = try!(config.acquire_package_cache_lock());

        for (name, dep) in try!(resolve_deps(&pkg, &config, source)) {
            rustdoc_flags.push("--extern-html-root-url".to_string());
            rustdoc_flags.push(format!(
                "{}=https://docs.rs/{}/{}",
                name.replace("-", "_"),
                dep.name(),
                dep.version()
            ));
        }

        let mut cargo_args = vec!["doc".to_owned(), "--lib".to_owned(), "--no-deps".to_owned()];
        if let Some(features) = &metadata.features {
            cargo_args.push("--features".to_owned());
            cargo_args.push(features.join(" "));
        }
        if metadata.all_features {
            cargo_args.push("--all-features".to_owned());
        }
        if metadata.no_default_features {
            cargo_args.push("--no-default-features".to_owned());
        }
        if let Some(target) = target {
            cargo_args.push("--target".into());
            cargo_args.push(target.into());
        }

        // TODO: We need to use build result here
        // FIXME: We also need build log (basically stderr message)
        let result = build
            .cargo()
            .env(
                "RUSTFLAGS",
                metadata
                    .rustc_args
                    .map(|args| args.join(""))
                    .unwrap_or("".to_owned()),
            )
            .env("RUSTDOCFLAGS", rustdoc_flags.join(" "))
            .args(&cargo_args)
            .run();

        // TODO: We need to return build result as well
        Ok(pkg)
    })?;

    Ok(pkg)
}

fn rustc_version(workspace: &Workspace, toolchain: &Toolchain) -> Result<String> {
    let res = Command::new(workspace, toolchain.rustc())
        .args(&["--version"])
        .log_output(false)
        .run_capture()?;

    if let Some(line) = res.stdout_lines().iter().next() {
        Ok(line.clone())
    } else {
        Err(::failure::err_msg(
            "invalid output returned by `rustc --version`",
        ))
    }
}

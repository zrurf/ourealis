//! Build script for the Ourealis service.
//!
//! Two jobs, in this order:
//!
//! 1. generate the protobuf bindings for the gRPC facade;
//! 2. make sure `web/dist` holds a built front-end, so the compile-time embed in
//!    `src/embed.rs` picks up real assets.
//!
//! The web step is best-effort on purpose. A machine without Node or pnpm still
//! has to be able to build and test the service, so a missing toolchain degrades
//! to a placeholder page; `OUREALIS_REQUIRE_WEB=1` turns that into a hard
//! failure for release builds.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

fn main() {
    println!("cargo:rerun-if-changed=proto");
    println!("cargo:rerun-if-env-changed=OUREALIS_SKIP_WEB");
    println!("cargo:rerun-if-env-changed=OUREALIS_REQUIRE_WEB");
    println!("cargo:rerun-if-env-changed=OUREALIS_PNPM");

    build_protos();
    sync_web_assets();
    emit_build_facts();
}

/// Exposes the workspace version and the build time to the binary.
///
/// The three crates share `[workspace.package] version`, so reading it once is
/// exact rather than approximate; the alternative — a constant in each crate —
/// would be three places to keep in step.
fn emit_build_facts() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let workspace = manifest.join("..").join("..").join("Cargo.toml");
    println!("cargo:rerun-if-changed={}", workspace.display());
    let version = std::fs::read_to_string(&workspace)
        .ok()
        .and_then(|text| {
            text.lines()
                .skip_while(|line| !line.trim_start().starts_with("[workspace.package]"))
                .find_map(|line| line.trim().strip_prefix("version").map(str::to_string))
        })
        .and_then(|rest| rest.split('"').nth(1).map(str::to_string))
        .unwrap_or_else(|| "0.0.0".to_string());
    println!("cargo:rustc-env=OUREALIS_WORKSPACE_VERSION={version}");

    let epoch_s = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|delta| delta.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=OUREALIS_BUILD_EPOCH_S={epoch_s}");
}

/// Generates the tonic/prost bindings for every service proto.
fn build_protos() {
    // A vendored protoc keeps the build working on machines without a protobuf
    // installation; an explicit `PROTOC` always wins so a distribution build can
    // pin its own compiler.
    if std::env::var_os("PROTOC").is_none()
        && let Ok(path) = protoc_bin_vendored::protoc_bin_path()
    {
        // SAFETY: the build script has not spawned any thread of its own.
        unsafe { std::env::set_var("PROTOC", path) };
    }

    // `common.proto` is pulled in by import, so it is not listed separately.
    let protos = [
        "proto/ourealis/api/v1/system.proto",
        "proto/ourealis/api/v1/map.proto",
        "proto/ourealis/api/v1/simulation.proto",
    ];
    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        .compile_protos(&protos, &["proto"])
        .expect("protobuf code generation failed");
}

/// Builds the front-end when needed and guarantees `web/dist` exists.
fn sync_web_assets() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let web = manifest.join("..").join("..").join("web");
    let dist = web.join("dist");
    let index = dist.join("index.html");

    for input in [
        "src",
        "public",
        "index.html",
        "package.json",
        "pnpm-lock.yaml",
        "vite.config.ts",
        "tsconfig.json",
        ".npmrc",
    ] {
        println!("cargo:rerun-if-changed={}", web.join(input).display());
    }
    // The embedded bytes come from this directory, so a change in it has to
    // recompile the crate that embeds them.
    println!("cargo:rerun-if-changed={}", dist.display());

    if !env_flag("OUREALIS_SKIP_WEB") && needs_web_build(&web, &index) {
        match run_web_build(&web) {
            Ok(()) => {}
            Err(error) => {
                // Say which of the two happened: an absent page becomes the
                // placeholder, an existing one is embedded as it was — which is a
                // stale page, not a placeholder, and the difference matters to
                // whoever is looking at the screen.
                let consequence = if index.is_file() {
                    "the previously built page will be embedded, so it may be out of date"
                } else {
                    "the embedded page will be the placeholder"
                };
                let message = format!("the web front-end was not rebuilt ({error}); {consequence}");
                if env_flag("OUREALIS_REQUIRE_WEB") {
                    panic!("{message}");
                }
                println!("cargo:warning={message}");
            }
        }
    }

    if !index.is_file()
        && let Err(error) = write_placeholder(&dist, &index)
    {
        panic!(
            "cannot create the placeholder web page in {}: {error}",
            dist.display()
        );
    }
}

fn env_flag(name: &str) -> bool {
    matches!(std::env::var(name), Ok(value) if value != "0" && !value.is_empty())
}

/// True when the built page is missing, is the placeholder, or is older than any
/// front-end input.
///
/// The placeholder check matters: its modification time is the moment it was written,
/// so without it a first build on a machine without pnpm would leave a page newer than
/// every input, and installing the toolchain afterwards would never trigger a real
/// build.
fn needs_web_build(web: &Path, index: &Path) -> bool {
    if !web.is_dir() {
        return false;
    }
    if is_placeholder(index) {
        return true;
    }
    let Ok(built_at) = fs::metadata(index).and_then(|meta| meta.modified()) else {
        return true;
    };
    let Some(newest) = newest_input(web) else {
        return false;
    };
    newest > built_at
}

/// Modification time of the most recently touched front-end input.
fn newest_input(web: &Path) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    let mut visit = |path: &Path| {
        if let Ok(time) = fs::metadata(path).and_then(|meta| meta.modified())
            && newest.map(|current| time > current).unwrap_or(true)
        {
            newest = Some(time);
        }
    };
    for entry in ["src", "public"] {
        walk(&web.join(entry), 8, &mut visit);
    }
    for entry in [
        "index.html",
        "package.json",
        "pnpm-lock.yaml",
        "vite.config.ts",
        "tsconfig.json",
        ".npmrc",
    ] {
        visit(&web.join(entry));
    }
    newest
}

fn walk(dir: &Path, depth: usize, visit: &mut impl FnMut(&Path)) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        // Build outputs and dependencies are never inputs.
        if name == "node_modules" || name == "dist" {
            continue;
        }
        if path.is_dir() {
            walk(&path, depth - 1, visit);
        } else {
            visit(&path);
        }
    }
}

/// Runs `pnpm install` (when needed) and `pnpm build`.
fn run_web_build(web: &Path) -> Result<(), String> {
    let pnpm = find_pnpm()?;
    if !web.join("node_modules").is_dir() {
        run(&pnpm, web, &["install", "--frozen-lockfile"])?;
    }
    run(&pnpm, web, &["run", "build"])
}

/// Locates a usable pnpm command.
///
/// Windows installs pnpm as a `.cmd` shim that `CreateProcess` will not run
/// directly, so the command goes through `cmd /C` there. `OUREALIS_PNPM` always
/// wins, which is what a CI image with a pinned pnpm should set.
fn find_pnpm() -> Result<String, String> {
    if let Ok(configured) = std::env::var("OUREALIS_PNPM")
        && !configured.is_empty()
    {
        return Ok(configured);
    }
    Ok("pnpm".to_string())
}

/// Runs one pnpm subcommand, returning its combined output on failure.
fn run(pnpm: &str, cwd: &Path, args: &[&str]) -> Result<(), String> {
    let mut command = if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(pnpm);
        command
    } else {
        Command::new(pnpm)
    };
    let output = command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run `{pnpm} {}`: {error}", args.join(" ")))?;
    if output.status.success() {
        return Ok(());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let tail: String = stdout
        .lines()
        .chain(stderr.lines())
        .rev()
        .take(12)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    Err(format!("`{pnpm} {}` failed:\n{tail}", args.join(" ")))
}

/// True when the page at `index` is the placeholder this script writes.
///
/// Matched on the marker the placeholder carries rather than on a timestamp: the
/// placeholder is rewritten whenever the page is missing, so it cannot record
/// anything more precise than which kind of page it is.
fn is_placeholder(index: &Path) -> bool {
    fs::read_to_string(index)
        .map(|text| text.contains("Web assets were not built"))
        .unwrap_or(false)
}

/// Writes the page served when the front-end was never built.
fn write_placeholder(dist: &Path, index: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dist)?;
    fs::write(index, PLACEHOLDER)
}

const PLACEHOLDER: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Ourealis service</title>
    <style>
      body { margin: 0; padding: 3rem 2rem; background: #0e0e10; color: #e8e8ea;
             font: 15px/1.6 -apple-system, "Segoe UI", Roboto, sans-serif; }
      main { max-width: 44rem; margin: 0 auto; }
      h1 { font-size: 1.35rem; font-weight: 600; margin: 0 0 1rem; }
      code { background: #1c1c20; padding: 0.15rem 0.35rem; border-radius: 4px; }
      p { margin: 0.6rem 0; color: #b6b6ba; }
      a { color: #5aa46a; }
    </style>
  </head>
  <body>
    <main>
      <h1>Web assets were not built</h1>
      <p>
        The API facades are running. This placeholder page means the front-end
        bundle was not embedded at build time.
      </p>
      <p>Build it with <code>pnpm install &amp;&amp; pnpm build</code> in <code>web/</code>, then
        rebuild the service, or run <code>cargo build -p ourealis</code> with Node and pnpm
        on <code>PATH</code>.</p>
      <p>The API starts at <a href="/api/v1/health">/api/v1/health</a>.</p>
    </main>
  </body>
</html>
"#;

// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

mod compression;
mod css;
mod script;

use std::fmt::Write as _;
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fmt, fs,
    path::{Component, Path, PathBuf},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use sha2::{Digest as _, Sha384};
use xxhash_rust::xxh64::xxh64;

/// Compiles one named asset set into the calling package's `OUT_DIR`.
///
/// `source_paths` contain CSS, JavaScript, and TypeScript inputs. `asset_paths` contain files to
/// copy unchanged. Every path is relative to the package `build.rs`, must remain below `src`, and
/// may name a file or directory. Symbolic links are rejected at every level. The build ID becomes
/// both the generated array identifier and the logical base name for compiled CSS/JavaScript.
///
/// CSS/JavaScript receive a complete xxHash64 filename fingerprint; every asset receives SHA-384
/// integrity. Compressible assets of at least 256 bytes also receive build-time gzip and Brotli
/// representations when a candidate saves at least 64 bytes and 10% over its more widely
/// supported fallback (identity for gzip, retained gzip for Brotli). One `assets.rs` contains the
/// complete ordered array.
///
/// # Errors
/// Returns an error when the ID or a path is invalid, Cargo has not supplied its build-script
/// environment, an input cannot be read, or compilation fails.
pub fn build(id: &str, source_paths: &[&str], asset_paths: &[&str]) -> Result<(), Error> {
    let manifest_dir = env_path("CARGO_MANIFEST_DIR")?;
    let out_dir = env_path("OUT_DIR")?;
    compile(&manifest_dir, &out_dir, id, source_paths, asset_paths).map(|_| ())
}

/// Cache policy emitted for a generated build output registered as an asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratedAssetCachePolicy {
    Revalidate,
    Immutable,
}

/// Registers one file already generated below the calling package's `OUT_DIR` as an asset set.
///
/// This is intended for deterministic build outputs such as a generated license report. It emits
/// `<id>_assets.rs`, preserves the identity file, and applies the same gzip/Brotli size gate as
/// [`build`]. Unlike `build` inputs, `source_path` is relative to `OUT_DIR`; absolute paths,
/// non-normal components, directories, and symbolic links are rejected.
///
/// # Errors
/// Returns an error when the ID, name, content type, or source path is invalid, Cargo has not
/// supplied `OUT_DIR`, or the generated file cannot be read or compressed.
pub fn build_generated_asset(
    id: &str,
    name: &str,
    content_type: &str,
    cache: GeneratedAssetCachePolicy,
    source_path: &str,
) -> Result<(), Error> {
    let out_dir = env_path("OUT_DIR")?;
    compile_generated_asset(&out_dir, id, name, content_type, cache, source_path)
}

fn compile_generated_asset(
    out_dir: &Path,
    id: &str,
    name: &str,
    content_type: &str,
    cache: GeneratedAssetCachePolicy,
    source_path: &str,
) -> Result<(), Error> {
    let id = BuildId::new(id)?;
    if !valid_asset_name(name)
        || !valid_content_type(content_type)
        || !valid_normal_relative_path(source_path)
    {
        return Err(Error::InvalidGeneratedAsset(name.to_owned()));
    }

    let source = resolve_generated_path(out_dir, source_path)?;
    let metadata = fs::symlink_metadata(&source).map_err(|source_error| Error::Io {
        path: source.clone(),
        source: source_error,
    })?;
    reject_symbolic_link(&source, metadata.file_type())?;
    if !metadata.is_file() {
        return Err(Error::InvalidGeneratedAsset(name.to_owned()));
    }
    let bytes = fs::read(&source).map_err(|source_error| Error::Io {
        path: source.clone(),
        source: source_error,
    })?;

    let mut binding = format!(
        "pub const {}: &[dimidiumlabs_ui::Asset] = &[\n",
        id.constant
    );
    push_asset_binding(
        out_dir,
        &mut binding,
        &mut BTreeSet::new(),
        BindingAsset {
            kind: format!(
                "dimidiumlabs_ui::AssetKind::Static {{ content_type: {content_type:?} }}"
            ),
            name: name.to_owned(),
            include_path: format!("/{source_path}"),
            bytes: &bytes,
            fingerprint: false,
            cache: match cache {
                GeneratedAssetCachePolicy::Revalidate => "dimidiumlabs_ui::CachePolicy::Revalidate",
                GeneratedAssetCachePolicy::Immutable => "dimidiumlabs_ui::CachePolicy::Immutable",
            },
            compressible: compressible_content_type(content_type),
        },
    )?;
    binding.push_str("];\n");
    write_output(
        &out_dir.join(format!("{}_assets.rs", id.basename)),
        &binding,
    )
}

fn resolve_generated_path(out_dir: &Path, source_path: &str) -> Result<PathBuf, Error> {
    let mut resolved = out_dir.to_owned();
    for component in Path::new(source_path).components() {
        let Component::Normal(component) = component else {
            return Err(Error::InvalidBuildPath(source_path.to_owned()));
        };
        resolved.push(component);
        let metadata = fs::symlink_metadata(&resolved).map_err(|source| Error::Io {
            path: resolved.clone(),
            source,
        })?;
        reject_symbolic_link(&resolved, metadata.file_type())?;
    }
    Ok(resolved)
}

fn valid_normal_relative_path(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn valid_asset_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.'))
        && name
            .split('/')
            .all(|component| !component.is_empty() && !matches!(component, "." | ".."))
}

fn valid_content_type(content_type: &str) -> bool {
    !content_type.is_empty()
        && content_type.is_ascii()
        && !content_type.bytes().any(|byte| byte.is_ascii_control())
}

/// The results produced by [`compile`]. This is mainly useful for build-tool integration tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub inputs: Vec<String>,
    pub rerun_if_changed: Vec<String>,
}

/// Compiles a named asset set in `manifest_dir` into `out_dir` without modifying source files.
///
/// Paths have the same meaning as in [`build`]. This function also prints
/// [`Output::rerun_if_changed`].
///
/// # Errors
/// Returns an error when inputs cannot be discovered or read, a symbolic link is encountered,
/// names are ambiguous, imports or composition need another file, or Lightning CSS rejects an
/// input.
pub fn compile(
    manifest_dir: &Path,
    out_dir: &Path,
    id: &str,
    source_paths: &[&str],
    asset_paths: &[&str],
) -> Result<Output, Error> {
    let id = BuildId::new(id)?;
    let source_paths = resolve_paths(manifest_dir, source_paths)?;
    let asset_paths = resolve_paths(manifest_dir, asset_paths)?;
    let inputs = discover(manifest_dir, &source_paths, &asset_paths)?;
    let static_assets = discover_static_assets(manifest_dir, &asset_paths)?;
    let mut global_styles = String::new();
    let mut component_styles = String::new();
    let mut modules = BTreeMap::new();
    let mut scripts = Vec::new();

    for input in &inputs {
        let source = fs::read_to_string(&input.path).map_err(|source| Error::Io {
            path: input.path.clone(),
            source,
        })?;
        match input.kind {
            InputKind::ModuleCss => {
                let (code, exports) = css::compile_module(&source, &input.logical_path)?;
                component_styles.push_str(&code);
                let module_name = input.module_name.clone().ok_or_else(|| Error::Css {
                    path: input.logical_path.clone(),
                    message: "module input has no module name".to_owned(),
                })?;
                modules.insert(module_name, exports);
            }
            InputKind::GlobalCss => {
                global_styles.push_str(&css::compile_global(&source, &input.logical_path)?);
            }
            InputKind::Script => scripts.push((
                input.logical_path.as_str(),
                source,
                input
                    .path
                    .extension()
                    .is_some_and(|extension| extension == "ts"),
            )),
        }
    }
    let script = script::compile_scripts(&scripts)?;
    let stylesheet = layered_stylesheet(&global_styles, &component_styles);

    fs::create_dir_all(out_dir).map_err(|source| Error::Io {
        path: out_dir.to_owned(),
        source,
    })?;
    let compressed_dir = out_dir.join("compressed");
    if compressed_dir.exists() {
        fs::remove_dir_all(&compressed_dir).map_err(|source| Error::Io {
            path: compressed_dir.clone(),
            source,
        })?;
    }
    write_output(&out_dir.join("stylesheet.css"), &stylesheet)?;
    write_output(
        &out_dir.join("css_modules.rs"),
        &css::render_bindings(&modules),
    )?;
    write_output(&out_dir.join("script.js"), &script)?;
    copy_static_assets(out_dir, &static_assets)?;
    write_output(
        &out_dir.join("assets.rs"),
        &assets_binding(out_dir, &id, &stylesheet, &script, &static_assets)?,
    )?;

    let mut rerun_if_changed = source_paths
        .iter()
        .chain(&asset_paths)
        .map(|path| {
            path.strip_prefix(manifest_dir)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect::<Vec<_>>();
    rerun_if_changed.extend(inputs.iter().map(|input| input.logical_path.clone()));
    rerun_if_changed.extend(static_assets.iter().map(|input| input.logical_path.clone()));
    rerun_if_changed.sort();
    rerun_if_changed.dedup();
    for directive in &rerun_if_changed {
        println!("cargo:rerun-if-changed={directive}");
    }
    Ok(Output {
        inputs: inputs
            .into_iter()
            .map(|input| input.logical_path)
            .chain(static_assets.into_iter().map(|input| input.logical_path))
            .collect(),
        rerun_if_changed,
    })
}

/// Errors returned while discovering or compiling build-script inputs.
#[derive(Debug)]
pub enum Error {
    MissingEnvironment(&'static str),
    InvalidBuildId(String),
    InvalidBuildPath(String),
    SymbolicLink {
        path: PathBuf,
    },
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Css {
        path: String,
        message: String,
    },
    JavaScript {
        path: String,
        message: String,
    },
    ModuleSyntax {
        path: String,
    },
    DuplicateModule {
        name: String,
        paths: Vec<String>,
    },
    DuplicateAssetName(String),
    InvalidGeneratedAsset(String),
    CrossFileComposes {
        path: String,
    },
    Import {
        path: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingEnvironment(name) => {
                write!(f, "missing build environment variable {name}")
            }
            Self::InvalidBuildId(id) => write!(f, "invalid asset build ID {id:?}"),
            Self::InvalidBuildPath(path) => {
                write!(f, "asset build path must stay below src: {path:?}")
            }
            Self::SymbolicLink { path } => write!(
                f,
                "symbolic links are not allowed in asset inputs: {}",
                path.display()
            ),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Css { path, message } | Self::JavaScript { path, message } => {
                write!(f, "{path}: {message}")
            }
            Self::ModuleSyntax { path } => write!(
                f,
                "{path}: ES module syntax is unsupported in classic scripts"
            ),
            Self::DuplicateModule { name, paths } => write!(
                f,
                "CSS module basename {name:?} is ambiguous: {}",
                paths.join(", ")
            ),
            Self::DuplicateAssetName(name) => write!(f, "duplicate generated asset name {name:?}"),
            Self::InvalidGeneratedAsset(name) => {
                write!(f, "invalid generated asset metadata for {name:?}")
            }
            Self::CrossFileComposes { path } => {
                write!(f, "{path}: composing from another file is unsupported")
            }
            Self::Import { path } => write!(f, "{path}: stylesheet imports are unsupported"),
        }
    }
}
impl std::error::Error for Error {}

#[derive(Debug)]
struct BuildId {
    constant: String,
    basename: String,
}

impl BuildId {
    fn new(id: &str) -> Result<Self, Error> {
        let mut bytes = id.bytes();
        if !bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
            || !bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(Error::InvalidBuildId(id.to_owned()));
        }
        Ok(Self {
            constant: id.replace('-', "_").to_ascii_uppercase(),
            basename: id.to_ascii_lowercase(),
        })
    }

    fn stylesheet_name(&self) -> String {
        format!("{}.css", self.basename)
    }

    fn script_name(&self) -> String {
        format!("{}.js", self.basename)
    }
}

#[derive(Debug)]
enum InputKind {
    GlobalCss,
    ModuleCss,
    Script,
}

#[derive(Debug)]
struct Input {
    path: PathBuf,
    logical_path: String,
    kind: InputKind,
    module_name: Option<String>,
}

#[derive(Debug)]
struct StaticInput {
    path: PathBuf,
    logical_path: String,
    relative_path: String,
}

fn env_path(name: &'static str) -> Result<PathBuf, Error> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or(Error::MissingEnvironment(name))
}

fn resolve_paths(manifest_dir: &Path, paths: &[&str]) -> Result<Vec<PathBuf>, Error> {
    paths
        .iter()
        .map(|path| {
            let relative = Path::new(path);
            let mut components = relative.components();
            if !matches!(components.next(), Some(Component::Normal(component)) if component == "src")
                || !components.all(|component| matches!(component, Component::Normal(_)))
            {
                return Err(Error::InvalidBuildPath((*path).to_owned()));
            }

            let mut resolved = manifest_dir.to_owned();
            for component in relative.components() {
                let Component::Normal(component) = component else {
                    unreachable!("build path components were validated above");
                };
                resolved.push(component);
                reject_symbolic_link(&resolved, fs::symlink_metadata(&resolved).map_err(|source| {
                    Error::Io {
                        path: resolved.clone(),
                        source,
                    }
                })?
                .file_type())?;
            }
            Ok(resolved)
        })
        .collect()
}

fn reject_symbolic_link(path: &Path, file_type: fs::FileType) -> Result<(), Error> {
    if file_type.is_symlink() {
        Err(Error::SymbolicLink {
            path: path.to_owned(),
        })
    } else {
        Ok(())
    }
}

fn discover(
    manifest_dir: &Path,
    roots: &[PathBuf],
    excluded_roots: &[PathBuf],
) -> Result<Vec<Input>, Error> {
    let mut files = Vec::new();
    for root in roots {
        collect_inputs(root, &mut files)?;
    }
    files.sort();
    files.dedup();
    files.retain(|path| !excluded_roots.iter().any(|root| path.starts_with(root)));
    let mut basenames: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut inputs = Vec::new();
    for path in files {
        let logical_path = path
            .strip_prefix(manifest_dir)
            .map_err(|_| Error::Css {
                path: path.display().to_string(),
                message: "input is outside the manifest directory".to_owned(),
            })?
            .to_string_lossy()
            .replace('\\', "/");
        let kind = classify_input(&path);
        let Some(kind) = kind else { continue };
        let module_name = matches!(kind, InputKind::ModuleCss).then(|| {
            css::rust_identifier(
                path.file_stem()
                    .and_then(|stem| Path::new(stem).file_stem())
                    .and_then(|stem| stem.to_str())
                    .expect("CSS module file name"),
            )
        });
        if let Some(name) = &module_name {
            basenames
                .entry(name.clone())
                .or_default()
                .push(logical_path.clone());
        }
        inputs.push(Input {
            path,
            logical_path,
            kind,
            module_name,
        });
    }
    for (name, paths) in basenames {
        if paths.len() > 1 {
            return Err(Error::DuplicateModule { name, paths });
        }
    }
    Ok(inputs)
}

fn discover_static_assets(
    manifest_dir: &Path,
    roots: &[PathBuf],
) -> Result<Vec<StaticInput>, Error> {
    let mut assets = Vec::new();
    for root in roots {
        let mut files = Vec::new();
        collect_static_files(root, &mut files)?;
        files.sort();
        files.dedup();
        for path in files {
            let logical_path = path
                .strip_prefix(manifest_dir)
                .map_err(|_| Error::Io {
                    path: path.clone(),
                    source: std::io::Error::other("static asset is outside the manifest"),
                })?
                .to_string_lossy()
                .replace('\\', "/");
            let relative_path = if root.is_file() {
                root.file_name()
                    .expect("validated static file has a name")
                    .to_string_lossy()
                    .into_owned()
            } else {
                path.strip_prefix(root)
                    .map_err(|_| Error::Io {
                        path: path.clone(),
                        source: std::io::Error::other(
                            "static asset is outside its configured root",
                        ),
                    })?
                    .to_string_lossy()
                    .replace('\\', "/")
            };
            assets.push(StaticInput {
                path,
                logical_path,
                relative_path,
            });
        }
    }
    assets.sort_by(|left, right| left.logical_path.cmp(&right.logical_path));
    Ok(assets)
}

fn classify_input(path: &Path) -> Option<InputKind> {
    let extension = path.extension()?.to_str()?;
    match extension {
        "css" if path.file_stem()?.to_str()?.ends_with(".module") => Some(InputKind::ModuleCss),
        "css" => Some(InputKind::GlobalCss),
        "js" => Some(InputKind::Script),
        "ts" if Path::new(path.file_stem()?).extension().is_none() => Some(InputKind::Script),
        _ => None,
    }
}

fn collect_inputs(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), Error> {
    if directory.is_file() {
        if matches!(
            directory
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("css" | "js" | "ts")
        ) {
            files.push(directory.to_owned());
        }
        return Ok(());
    }
    let entries = fs::read_dir(directory).map_err(|source| Error::Io {
        path: directory.to_owned(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            path: directory.to_owned(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        reject_symbolic_link(&path, file_type)?;
        if file_type.is_dir() {
            collect_inputs(&path, files)?;
        } else if matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("css" | "js" | "ts")
        ) {
            files.push(path);
        }
    }
    Ok(())
}

fn collect_static_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), Error> {
    if directory.is_file() {
        files.push(directory.to_owned());
        return Ok(());
    }
    let entries = fs::read_dir(directory).map_err(|source| Error::Io {
        path: directory.to_owned(),
        source,
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| Error::Io {
            path: directory.to_owned(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
        reject_symbolic_link(&path, file_type)?;
        if file_type.is_dir() {
            collect_static_files(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

fn layered_stylesheet(global_styles: &str, component_styles: &str) -> String {
    let mut stylesheet = String::from("@layer global,components;");
    if !global_styles.is_empty() {
        stylesheet.push_str("@layer global{");
        stylesheet.push_str(global_styles);
        stylesheet.push('}');
    }
    if !component_styles.is_empty() {
        stylesheet.push_str("@layer components{");
        stylesheet.push_str(component_styles);
        stylesheet.push('}');
    }
    stylesheet
}

fn assets_binding(
    out_dir: &Path,
    id: &BuildId,
    stylesheet: &str,
    script: &str,
    static_assets: &[StaticInput],
) -> Result<String, Error> {
    let mut binding = format!(
        "pub const {}: &[dimidiumlabs_ui::Asset] = &[\n",
        id.constant
    );
    let mut names = BTreeSet::new();
    push_asset_binding(
        out_dir,
        &mut binding,
        &mut names,
        BindingAsset {
            kind: "dimidiumlabs_ui::AssetKind::Stylesheet".to_owned(),
            name: id.stylesheet_name(),
            include_path: "/stylesheet.css".to_owned(),
            bytes: stylesheet.as_bytes(),
            fingerprint: true,
            cache: "dimidiumlabs_ui::CachePolicy::Immutable",
            compressible: true,
        },
    )?;
    if !script.is_empty() {
        push_asset_binding(
            out_dir,
            &mut binding,
            &mut names,
            BindingAsset {
                kind: "dimidiumlabs_ui::AssetKind::Script".to_owned(),
                name: id.script_name(),
                include_path: "/script.js".to_owned(),
                bytes: script.as_bytes(),
                fingerprint: true,
                cache: "dimidiumlabs_ui::CachePolicy::Immutable",
                compressible: true,
            },
        )?;
    }
    for asset in static_assets {
        let bytes = fs::read(&asset.path).map_err(|source| Error::Io {
            path: asset.path.clone(),
            source,
        })?;
        let content_type = static_content_type(&asset.relative_path);
        push_asset_binding(
            out_dir,
            &mut binding,
            &mut names,
            BindingAsset {
                kind: format!(
                    "dimidiumlabs_ui::AssetKind::Static {{ content_type: {content_type:?} }}"
                ),
                name: asset.relative_path.clone(),
                include_path: format!("/assets/{}", asset.relative_path),
                bytes: &bytes,
                fingerprint: false,
                cache: static_cache_policy(&asset.relative_path),
                compressible: compressible_content_type(content_type),
            },
        )?;
    }
    binding.push_str("];\n");
    Ok(binding)
}

struct BindingAsset<'a> {
    kind: String,
    name: String,
    include_path: String,
    bytes: &'a [u8],
    fingerprint: bool,
    cache: &'static str,
    compressible: bool,
}

fn push_asset_binding(
    out_dir: &Path,
    binding: &mut String,
    names: &mut BTreeSet<String>,
    asset: BindingAsset<'_>,
) -> Result<(), Error> {
    let fingerprinted_name = if asset.fingerprint {
        fingerprinted_name(&asset.name, xxh64(asset.bytes, 0))
    } else {
        asset.name.clone()
    };
    if !names.insert(asset.name.clone()) {
        return Err(Error::DuplicateAssetName(asset.name));
    }
    if fingerprinted_name != asset.name && !names.insert(fingerprinted_name.clone()) {
        return Err(Error::DuplicateAssetName(fingerprinted_name));
    }
    let identity_integrity = integrity(asset.bytes);
    let variants = if asset.compressible {
        compression::compress(asset.bytes).map_err(|source| Error::Io {
            path: PathBuf::from(&asset.name),
            source,
        })?
    } else {
        compression::Variants {
            gzip: None,
            brotli: None,
        }
    };
    let BindingAsset {
        kind,
        name,
        include_path,
        cache,
        ..
    } = asset;
    write!(
        binding,
        "    dimidiumlabs_ui::Asset::new(\n        {kind},\n        {name:?},\n        {fingerprinted_name:?},\n        {cache},\n        include_bytes!(concat!(env!(\"OUT_DIR\"), {include_path:?})),\n        {identity_integrity:?},\n    )"
    )
    .expect("writing to String cannot fail");
    if let Some(bytes) = variants.gzip {
        let path = write_compressed(out_dir, &include_path, "gz", &bytes)?;
        let etag = integrity(&bytes);
        write!(
            binding,
            ".with_gzip(include_bytes!(concat!(env!(\"OUT_DIR\"), {path:?})), {etag:?})"
        )
        .expect("writing to String cannot fail");
    }
    if let Some(bytes) = variants.brotli {
        let path = write_compressed(out_dir, &include_path, "br", &bytes)?;
        let etag = integrity(&bytes);
        write!(
            binding,
            ".with_brotli(include_bytes!(concat!(env!(\"OUT_DIR\"), {path:?})), {etag:?})"
        )
        .expect("writing to String cannot fail");
    }
    binding.push_str(",\n");
    Ok(())
}

fn write_compressed(
    out_dir: &Path,
    include_path: &str,
    extension: &str,
    bytes: &[u8],
) -> Result<String, Error> {
    let relative = format!(
        "compressed/{}.{extension}",
        include_path.trim_start_matches('/')
    );
    let destination = out_dir.join(&relative);
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|source| Error::Io {
            path: parent.to_owned(),
            source,
        })?;
    }
    fs::write(&destination, bytes).map_err(|source| Error::Io {
        path: destination,
        source,
    })?;
    Ok(format!("/{relative}"))
}

fn compressible_content_type(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || content_type.starts_with("application/json")
        || content_type.starts_with("application/manifest+json")
        || content_type.starts_with("application/javascript")
        || content_type.starts_with("application/xml")
        || content_type.starts_with("image/svg+xml")
}

fn fingerprinted_name(name: &str, fingerprint: u64) -> String {
    let (directory, filename) = name
        .rsplit_once('/')
        .map_or(("", name), |(directory, filename)| (directory, filename));
    let fingerprinted = filename.rsplit_once('.').map_or_else(
        || format!("{filename}.{fingerprint:016x}"),
        |(stem, extension)| format!("{stem}.{fingerprint:016x}.{extension}"),
    );
    if directory.is_empty() {
        fingerprinted
    } else {
        format!("{directory}/{fingerprinted}")
    }
}

fn static_cache_policy(name: &str) -> &'static str {
    if Path::new(name)
        .extension()
        .is_some_and(|extension| extension == "woff2")
    {
        "dimidiumlabs_ui::CachePolicy::Immutable"
    } else {
        "dimidiumlabs_ui::CachePolicy::Revalidate"
    }
}

fn static_content_type(name: &str) -> &'static str {
    match Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
    {
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("svg") => "image/svg+xml",
        Some("txt") => "text/plain; charset=utf-8",
        Some("webmanifest") => "application/manifest+json",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn copy_static_assets(out_dir: &Path, assets: &[StaticInput]) -> Result<(), Error> {
    for asset in assets {
        let destination = out_dir.join("assets").join(&asset.relative_path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|source| Error::Io {
                path: parent.to_owned(),
                source,
            })?;
        }
        fs::copy(&asset.path, &destination).map_err(|source| Error::Io {
            path: destination,
            source,
        })?;
    }
    Ok(())
}

fn integrity(bytes: &[u8]) -> String {
    let digest = Sha384::digest(bytes);
    format!("sha384-{}", STANDARD.encode(digest))
}

fn write_output(path: &Path, content: &str) -> Result<(), Error> {
    fs::write(path, content).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn discovers_only_supported_inputs_in_logical_order() {
        let project = tempdir().unwrap();
        fs::create_dir_all(project.path().join("src/z")).unwrap();
        fs::create_dir_all(project.path().join("src/styles")).unwrap();
        fs::write(project.path().join("src/z/a.module.css"), ".a {}").unwrap();
        fs::write(project.path().join("src/styles/site.css"), "body {}").unwrap();
        fs::write(project.path().join("src/ignored.css"), ".ignored {}").unwrap();
        fs::write(project.path().join("src/z/first.js"), "window.first = 1;").unwrap();
        fs::write(project.path().join("src/z/second.ts"), "window.second = 2;").unwrap();
        fs::write(
            project.path().join("src/z/types.d.ts"),
            "interface Value {}",
        )
        .unwrap();
        fs::write(
            project.path().join("src/z/view.tsx"),
            "const view = <div />;",
        )
        .unwrap();
        fs::create_dir_all(project.path().join("src/assets/nested")).unwrap();
        fs::write(project.path().join("src/assets/app.js"), "static script").unwrap();
        fs::write(project.path().join("src/assets/nested/icon.bin"), b"icon").unwrap();
        let inputs = discover(
            project.path(),
            &[
                project.path().join("src/styles"),
                project.path().join("src/z"),
            ],
            &[project.path().join("src/assets")],
        )
        .unwrap();
        let paths = inputs
            .into_iter()
            .map(|input| input.logical_path)
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            [
                "src/styles/site.css",
                "src/z/a.module.css",
                "src/z/first.js",
                "src/z/second.ts"
            ]
        );
        assert!(!paths.contains(&"src/z/types.d.ts".to_owned()));
        assert!(!paths.contains(&"src/z/view.tsx".to_owned()));

        let static_assets =
            discover_static_assets(project.path(), &[project.path().join("src/assets")]).unwrap();
        assert_eq!(
            static_assets
                .iter()
                .map(|asset| asset.relative_path.as_str())
                .collect::<Vec<_>>(),
            ["app.js", "nested/icon.bin"]
        );
    }

    #[test]
    fn rejects_duplicate_modules() {
        let project = tempdir().unwrap();
        fs::create_dir_all(project.path().join("src/a")).unwrap();
        fs::create_dir_all(project.path().join("src/b")).unwrap();
        fs::write(project.path().join("src/a/card.module.css"), ".a {}").unwrap();
        fs::write(project.path().join("src/b/card.module.css"), ".b {}").unwrap();
        assert!(matches!(
            discover(project.path(), &[project.path().join("src")], &[]),
            Err(Error::DuplicateModule { .. })
        ));
    }

    #[test]
    fn component_layer_follows_and_overrides_global_layer() {
        let stylesheet = layered_stylesheet(
            "@font-face{font-family:test;src:url(test.woff2)}p{margin:1rem}",
            ".card{margin:0}",
        );
        assert_eq!(
            stylesheet,
            "@layer global,components;@layer global{@font-face{font-family:test;src:url(test.woff2)}p{margin:1rem}}@layer components{.card{margin:0}}",
        );
        assert!(css::compile_global(&stylesheet, "generated.css").is_ok());
    }

    #[test]
    fn registers_generated_out_dir_files_with_the_same_compression_policy() {
        let output = tempdir().unwrap();
        let json = format!("[{}]", "\"generated license text\",".repeat(10_000));
        fs::write(output.path().join("licenses.json"), &json).unwrap();

        compile_generated_asset(
            output.path(),
            "LICENSES",
            "licenses.json",
            "application/json; charset=utf-8",
            GeneratedAssetCachePolicy::Revalidate,
            "licenses.json",
        )
        .unwrap();

        let bindings = fs::read_to_string(output.path().join("licenses_assets.rs")).unwrap();
        assert!(bindings.starts_with("pub const LICENSES: &[dimidiumlabs_ui::Asset]"));
        assert!(bindings.contains("/licenses.json"));
        assert!(bindings.contains("/compressed/licenses.json.gz"));
        assert!(bindings.contains("/compressed/licenses.json.br"));
        assert!(output.path().join("compressed/licenses.json.gz").is_file());
        assert!(output.path().join("compressed/licenses.json.br").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_generated_out_dir_symlinks() {
        use std::os::unix::fs::symlink;

        let output = tempdir().unwrap();
        let outside = tempdir().unwrap();
        fs::write(outside.path().join("licenses.json"), "[]").unwrap();
        symlink(
            outside.path().join("licenses.json"),
            output.path().join("licenses.json"),
        )
        .unwrap();

        assert!(matches!(
            compile_generated_asset(
                output.path(),
                "LICENSES",
                "licenses.json",
                "application/json",
                GeneratedAssetCachePolicy::Revalidate,
                "licenses.json",
            ),
            Err(Error::SymbolicLink { .. })
        ));
    }

    #[test]
    fn emits_only_material_precompressed_text_representations() {
        let project = tempdir().unwrap();
        let output = tempdir().unwrap();
        fs::create_dir_all(project.path().join("src/assets")).unwrap();
        let json = format!(
            "[{}]",
            "\"abcdefghijklmnopqrstuvwxyz0123456789\",".repeat(10_000)
        );
        fs::write(project.path().join("src/assets/data.json"), &json).unwrap();
        fs::write(project.path().join("src/assets/image.png"), vec![0; 20_000]).unwrap();

        compile(
            project.path(),
            output.path(),
            "APPLICATION",
            &[],
            &["src/assets"],
        )
        .unwrap();

        let bindings = fs::read_to_string(output.path().join("assets.rs")).unwrap();
        assert!(bindings.contains(".with_gzip("));
        assert!(bindings.contains(".with_brotli("));
        assert!(bindings.contains("/compressed/assets/data.json.gz"));
        assert!(bindings.contains("/compressed/assets/data.json.br"));
        assert!(!bindings.contains("/compressed/assets/image.png"));
        assert!(
            output
                .path()
                .join("compressed/assets/data.json.gz")
                .is_file()
        );
        assert!(
            output
                .path()
                .join("compressed/assets/data.json.br")
                .is_file()
        );
        assert!(
            !output
                .path()
                .join("compressed/assets/image.png.gz")
                .exists()
        );
    }

    #[test]
    fn output_is_checkout_independent() {
        let first = tempdir().unwrap();
        let second = tempdir().unwrap();
        let first_out = tempdir().unwrap();
        let second_out = tempdir().unwrap();
        for project in [&first, &second] {
            fs::create_dir_all(project.path().join("src/components")).unwrap();
            fs::write(
                project.path().join("src/components/card.module.css"),
                ".card { color: red; }",
            )
            .unwrap();
            fs::write(
                project.path().join("src/components/script.js"),
                "window.value = 1;",
            )
            .unwrap();
            fs::create_dir_all(project.path().join("src/assets/icons")).unwrap();
            fs::write(
                project.path().join("src/assets/icons/app.bin"),
                b"static bytes",
            )
            .unwrap();
        }
        compile(
            first.path(),
            first_out.path(),
            "APPLICATION",
            &["src"],
            &["src/assets"],
        )
        .unwrap();
        compile(
            second.path(),
            second_out.path(),
            "APPLICATION",
            &["src"],
            &["src/assets"],
        )
        .unwrap();
        for output in ["stylesheet.css", "css_modules.rs", "script.js", "assets.rs"] {
            assert_eq!(
                fs::read_to_string(first_out.path().join(output)).unwrap(),
                fs::read_to_string(second_out.path().join(output)).unwrap(),
            );
        }
        assert_eq!(
            fs::read(first_out.path().join("assets/icons/app.bin")).unwrap(),
            fs::read(second_out.path().join("assets/icons/app.bin")).unwrap(),
        );
        let bindings = fs::read_to_string(first_out.path().join("assets.rs")).unwrap();
        assert!(bindings.contains("pub const APPLICATION"));
        assert!(bindings.contains("application.css"));
        assert!(bindings.contains("application.js"));
        assert!(bindings.contains("icons/app.bin"));
        assert!(bindings.contains("sha384-"));
        let stylesheet = fs::read(first_out.path().join("stylesheet.css")).unwrap();
        assert!(bindings.contains(&format!("{:016x}", xxh64(&stylesheet, 0))));
        assert!(bindings.contains(&integrity(&stylesheet)));
        assert!(bindings.contains(&integrity(b"static bytes")));
        assert!(bindings.contains("\"icons/app.bin\",\n        \"icons/app.bin\""));
    }

    #[test]
    fn compilation_writes_all_outputs_and_rerun_directives() {
        let project = tempdir().unwrap();
        let output = tempdir().unwrap();
        fs::create_dir_all(project.path().join("src/styles")).unwrap();
        fs::write(
            project.path().join("src/card.module.css"),
            ".card { color: red; }",
        )
        .unwrap();
        fs::write(
            project.path().join("src/styles/site.css"),
            "body { margin: 0px; }",
        )
        .unwrap();
        let result = compile(project.path(), output.path(), "APPLICATION", &["src"], &[]).unwrap();
        let stylesheet = fs::read_to_string(output.path().join("stylesheet.css")).unwrap();
        let bindings = fs::read_to_string(output.path().join("css_modules.rs")).unwrap();
        assert!(!stylesheet.contains(".card{") && stylesheet.contains("body{margin:0"));
        assert!(stylesheet.starts_with("@layer global,components;"));
        assert!(stylesheet.contains("@layer global{body{margin:0}"));
        assert!(stylesheet.contains("@layer components{"));
        assert!(bindings.contains("pub mod card"));
        assert_eq!(
            fs::read_to_string(output.path().join("script.js")).unwrap(),
            ""
        );
        let assets = fs::read_to_string(output.path().join("assets.rs")).unwrap();
        assert!(assets.starts_with("pub const APPLICATION: &[dimidiumlabs_ui::Asset] = &["));
        assert_eq!(assets.matches("dimidiumlabs_ui::Asset::new(").count(), 1);
        assert!(assets.contains("dimidiumlabs_ui::AssetKind::Stylesheet"));
        assert!(assets.contains("\"application.css\""));
        assert!(!assets.contains("AssetKind::Script"));
        assert_eq!(
            result.rerun_if_changed,
            ["src", "src/card.module.css", "src/styles/site.css"]
        );
    }

    #[test]
    fn build_id_defines_the_array_and_stylesheet_names() {
        let project = tempdir().unwrap();
        let output = tempdir().unwrap();
        fs::create_dir_all(project.path().join("src/styles")).unwrap();
        fs::write(project.path().join("src/styles/site.css"), "body {}").unwrap();
        compile(
            project.path(),
            output.path(),
            "FOUNDATION",
            &["src/styles"],
            &[],
        )
        .unwrap();
        let assets = fs::read_to_string(output.path().join("assets.rs")).unwrap();
        assert!(assets.starts_with("pub const FOUNDATION: &[dimidiumlabs_ui::Asset] = &["));
        assert!(assets.contains("\"foundation.css\""));
    }

    #[test]
    fn rejects_static_name_colliding_with_generated_application_asset() {
        let project = tempdir().unwrap();
        let output = tempdir().unwrap();
        fs::create_dir_all(project.path().join("src/assets")).unwrap();
        fs::write(project.path().join("src/assets/application.css"), "static").unwrap();
        assert!(matches!(
            compile(
                project.path(),
                output.path(),
                "APPLICATION",
                &["src"],
                &["src/assets"],
            ),
            Err(Error::DuplicateAssetName(name)) if name == "application.css"
        ));
    }

    #[test]
    fn rejects_invalid_build_ids_and_paths_outside_src() {
        let project = tempdir().unwrap();
        let output = tempdir().unwrap();
        assert!(matches!(
            compile(project.path(), output.path(), "bad id", &[], &[]),
            Err(Error::InvalidBuildId(id)) if id == "bad id"
        ));
        assert!(matches!(
            compile(
                project.path(),
                output.path(),
                "VALID",
                &["../outside"],
                &[],
            ),
            Err(Error::InvalidBuildPath(path)) if path == "../outside"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_cyclic_source_directory_symlinks() {
        use std::os::unix::fs::symlink;

        let project = tempdir().unwrap();
        let output = tempdir().unwrap();
        let styles = project.path().join("src/styles");
        fs::create_dir_all(&styles).unwrap();
        fs::write(styles.join("site.css"), "body {}").unwrap();
        symlink(&styles, styles.join("loop")).unwrap();

        assert!(matches!(
            compile(project.path(), output.path(), "VALID", &["src/styles"], &[]),
            Err(Error::SymbolicLink { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_static_asset_symlinks_outside_src() {
        use std::os::unix::fs::symlink;

        let project = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let output = tempdir().unwrap();
        let assets = project.path().join("src/assets");
        fs::create_dir_all(&assets).unwrap();
        let secret = outside.path().join("secret.bin");
        fs::write(&secret, b"outside").unwrap();
        symlink(secret, assets.join("secret.bin")).unwrap();

        assert!(matches!(
            compile(project.path(), output.path(), "VALID", &[], &["src/assets"]),
            Err(Error::SymbolicLink { .. })
        ));

        fs::remove_file(assets.join("secret.bin")).unwrap();
        symlink(outside.path(), project.path().join("src/external-assets")).unwrap();
        assert!(matches!(
            compile(
                project.path(),
                output.path(),
                "VALID",
                &[],
                &["src/external-assets"],
            ),
            Err(Error::SymbolicLink { .. })
        ));
    }
}

//! Filesystem-backed source-map location resolution.
//!
//! All line and column values accepted or returned by this crate are zero-based.

use std::fs::File;
use std::io::BufReader;
use std::path::{Component, Path, PathBuf, MAIN_SEPARATOR};

use sourcemap::DecodedMap;
use thiserror::Error;

/// An error loading or querying source maps.
#[derive(Debug, Error)]
pub enum SourceMapError {
    #[error("source map not found: {0}")]
    NotFound(String),
    #[error("invalid source map: {0}")]
    Invalid(String),
}

/// An owned location in an original source file.
///
/// `line` and `column` are zero-based. `path` is an absolute, lexically
/// normalized filesystem path; the source file does not need to exist.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OriginalLocation {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
    pub name: Option<String>,
}

/// An owned location in generated code.
///
/// `line` and `column` are zero-based. `path` is a normalized remote path with
/// a leading slash and forward slashes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedLocation {
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub name: Option<String>,
}

#[derive(Debug)]
struct Mapping {
    generated_line: u32,
    generated_column: u32,
    original_path: PathBuf,
    original_line: u32,
    original_column: u32,
    name: Option<String>,
}

#[derive(Debug)]
struct LoadedMap {
    generated_key: String,
    generated_path: String,
    mappings: Vec<Mapping>,
}

/// A collection of loaded source maps.
///
/// [`SourceMaps::new`] and [`Default`] intentionally create an empty resolver.
#[derive(Debug, Default)]
pub struct SourceMaps {
    maps: Vec<LoadedMap>,
}

impl SourceMaps {
    /// Creates an empty resolver.
    pub fn new() -> Self {
        Self::default()
    }

    /// Loads `<workspace_root>/BP/scripts/main.js.map`.
    ///
    /// `<workspace_root>/BP` is used as the generated root, so the map is
    /// addressed remotely as `/scripts/main.js`.
    pub fn from_workspace(workspace_root: impl AsRef<Path>) -> Result<Self, SourceMapError> {
        let workspace_root = absolute_normalized(workspace_root.as_ref())?;
        let generated_root = workspace_root.join("BP");
        let map_path = generated_root.join("scripts").join("main.js.map");
        Self::from_map_file(map_path, generated_root)
    }

    /// Loads one regular v3 source map from an explicit path.
    ///
    /// The generated remote path is the map path relative to `generated_root`,
    /// with the final `.map` suffix removed. Original sources are resolved
    /// relative to the map's directory after the decoder expands `sourceRoot`.
    pub fn from_map_file(
        map_path: impl AsRef<Path>,
        generated_root: impl AsRef<Path>,
    ) -> Result<Self, SourceMapError> {
        let map_path = absolute_normalized(map_path.as_ref())?;
        let generated_root = absolute_normalized(generated_root.as_ref())?;
        let generated_relative = map_path.strip_prefix(&generated_root).map_err(|_| {
            SourceMapError::Invalid(format!(
                "map '{}' is not beneath generated root '{}'",
                map_path.display(),
                generated_root.display()
            ))
        })?;
        let generated_relative = remove_map_suffix(generated_relative)?;
        let generated_key = normalize_generated_path(&generated_relative.to_string_lossy());
        if generated_key.is_empty() {
            return Err(SourceMapError::Invalid(format!(
                "map '{}' does not identify a generated file",
                map_path.display()
            )));
        }

        let file = File::open(&map_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                SourceMapError::NotFound(format!("'{}'", map_path.display()))
            } else {
                SourceMapError::Invalid(format!("could not read '{}': {error}", map_path.display()))
            }
        })?;
        let decoded = sourcemap::decode(BufReader::new(file)).map_err(|error| {
            SourceMapError::Invalid(format!(
                "could not decode '{}': {error}",
                map_path.display()
            ))
        })?;
        let map = match decoded {
            DecodedMap::Regular(map) => map,
            _ => {
                return Err(SourceMapError::Invalid(format!(
                    "'{}' is not a regular source map",
                    map_path.display()
                )))
            }
        };

        let map_directory = map_path.parent().ok_or_else(|| {
            SourceMapError::Invalid(format!(
                "map '{}' has no parent directory",
                map_path.display()
            ))
        })?;
        let mappings = map
            .tokens()
            .filter_map(|token| {
                let source = token.get_source()?;
                Some(Mapping {
                    generated_line: token.get_dst_line(),
                    generated_column: token.get_dst_col(),
                    original_path: resolve_source_path(map_directory, source),
                    original_line: token.get_src_line(),
                    original_column: token.get_src_col(),
                    name: token.get_name().map(str::to_owned),
                })
            })
            .collect();

        Ok(Self {
            maps: vec![LoadedMap {
                generated_path: format!("/{generated_key}"),
                generated_key,
                mappings,
            }],
        })
    }

    /// Resolves a generated zero-based location to an original source.
    ///
    /// On the requested generated line, this chooses the least mapping column
    /// greater than or equal to `column` (LUB), falling back to the greatest
    /// mapping column less than `column` (GLB). It never crosses a generated
    /// line.
    pub fn generated_to_original(
        &self,
        generated_path: impl AsRef<str>,
        line: u32,
        column: u32,
    ) -> Result<OriginalLocation, SourceMapError> {
        let input = generated_path.as_ref();
        let key = normalize_generated_path(input);
        let loaded = self
            .maps
            .iter()
            .find(|loaded| loaded.generated_key == key)
            .ok_or_else(|| SourceMapError::NotFound(format!("generated module '{input}'")))?;

        let mapping = best_mapping(
            loaded
                .mappings
                .iter()
                .filter(|mapping| mapping.generated_line == line),
            column,
            |mapping| mapping.generated_column,
        )
        .ok_or_else(|| {
            SourceMapError::NotFound(format!(
                "mapping for '{}' at {line}:{column}",
                loaded.generated_path
            ))
        })?;

        Ok(OriginalLocation {
            path: mapping.original_path.clone(),
            line: mapping.original_line,
            column: mapping.original_column,
            name: mapping.name.clone(),
        })
    }

    /// Resolves an original zero-based location to its deterministic best
    /// generated location.
    ///
    /// On the requested original line, this chooses source-column LUB and then
    /// GLB. Original paths are matched after absolute lexical normalization and
    /// do not need to exist.
    pub fn original_to_generated(
        &self,
        original_path: impl AsRef<Path>,
        line: u32,
        column: u32,
    ) -> Result<GeneratedLocation, SourceMapError> {
        let original_path = absolute_normalized(original_path.as_ref())?;

        let best = self.maps.iter().find_map(|loaded| {
            best_mapping(
                loaded.mappings.iter().filter(|mapping| {
                    paths_match(&mapping.original_path, &original_path)
                        && mapping.original_line == line
                }),
                column,
                |mapping| mapping.original_column,
            )
            .map(|mapping| (loaded, mapping))
        });
        let (loaded, mapping) = best.ok_or_else(|| {
            SourceMapError::NotFound(format!(
                "mapping for '{}' at {line}:{column}",
                original_path.display()
            ))
        })?;

        Ok(GeneratedLocation {
            path: loaded.generated_path.clone(),
            line: mapping.generated_line,
            column: mapping.generated_column,
            name: mapping.name.clone(),
        })
    }
}

fn best_mapping<'a, I, F>(mappings: I, column: u32, get_column: F) -> Option<&'a Mapping>
where
    I: Iterator<Item = &'a Mapping>,
    F: Fn(&Mapping) -> u32,
{
    let mut lub = None;
    let mut glb = None;

    for mapping in mappings {
        let mapping_column = get_column(mapping);
        if mapping_column >= column {
            if lub.is_none_or(|current| get_column(current) > mapping_column) {
                lub = Some(mapping);
            }
        } else if glb.is_none_or(|current| get_column(current) < mapping_column) {
            glb = Some(mapping);
        }
    }

    lub.or(glb)
}

fn remove_map_suffix(path: &Path) -> Result<PathBuf, SourceMapError> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| SourceMapError::Invalid(format!("invalid map path '{}'", path.display())))?;
    let generated_name = file_name.strip_suffix(".map").ok_or_else(|| {
        SourceMapError::Invalid(format!("map path '{}' must end in .map", path.display()))
    })?;
    let mut generated = path.to_path_buf();
    generated.set_file_name(generated_name);
    Ok(generated)
}

fn normalize_generated_path(path: &str) -> String {
    let mut parts = Vec::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    parts.join("/")
}

fn resolve_source_path(map_directory: &Path, source: &str) -> PathBuf {
    let source = native_path(source);
    if source.is_absolute() {
        lexical_normalize(&source)
    } else {
        lexical_normalize(&map_directory.join(source))
    }
}

fn native_path(path: &str) -> PathBuf {
    path.chars()
        .map(|character| {
            if character == '/' || character == '\\' {
                MAIN_SEPARATOR
            } else {
                character
            }
        })
        .collect::<String>()
        .into()
}

fn absolute_normalized(path: &Path) -> Result<PathBuf, SourceMapError> {
    let path = native_path(&path.to_string_lossy());
    if path.is_absolute() {
        Ok(lexical_normalize(&path))
    } else {
        let current_directory = std::env::current_dir().map_err(|error| {
            SourceMapError::Invalid(format!("could not determine current directory: {error}"))
        })?;
        Ok(lexical_normalize(&current_directory.join(path)))
    }
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                Some(Component::ParentDir) | None if !path.is_absolute() => {
                    normalized.push(component);
                }
                _ => {}
            },
            _ => normalized.push(component),
        }
    }
    normalized
}

fn paths_match(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let unique = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir()
                .join(format!("mc-source-maps-{}-{unique}", std::process::id()));
            let scripts = root.join("BP").join("scripts");
            fs::create_dir_all(&scripts).expect("create test workspace");
            Self { root }
        }

        fn map_path(&self) -> PathBuf {
            self.root.join("BP").join("scripts").join("main.js.map")
        }

        fn write_valid_map(&self) {
            fs::write(
                self.map_path(),
                r#"{
                    "version": 3,
                    "file": "main.js",
                    "sourceRoot": "../../",
                    "sources": ["src/main.ts"],
                    "names": ["first", "second"],
                    "mappings": "EAAIA,MAAMC;AACT"
                }"#,
            )
            .expect("write source map");
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn consumes_exact_workspace_map_and_resolves_both_directions() {
        let workspace = TestWorkspace::new();
        workspace.write_valid_map();
        let maps = SourceMaps::from_workspace(&workspace.root).expect("load workspace map");

        let original = maps
            .generated_to_original("/scripts/main.js", 0, 2)
            .expect("resolve generated location");
        assert_eq!(original.path, workspace.root.join("src").join("main.ts"));
        assert_eq!((original.line, original.column), (0, 4));
        assert_eq!(original.name.as_deref(), Some("first"));

        let generated = maps
            .original_to_generated(workspace.root.join("src").join("main.ts"), 0, 4)
            .expect("resolve original location");
        assert_eq!(generated.path, "/scripts/main.js");
        assert_eq!((generated.line, generated.column), (0, 2));
        assert_eq!(generated.name.as_deref(), Some("first"));
    }

    #[test]
    fn normalizes_generated_and_original_paths() {
        let workspace = TestWorkspace::new();
        workspace.write_valid_map();
        let maps = SourceMaps::from_workspace(&workspace.root).expect("load workspace map");

        let original = maps
            .generated_to_original(r"\scripts\.\ignored\..\main.js", 1, 0)
            .expect("resolve normalized generated path");
        assert_eq!((original.line, original.column), (1, 1));

        let noncanonical_source = workspace
            .root
            .join("src")
            .join("nested")
            .join("..")
            .join("main.ts");
        let generated = maps
            .original_to_generated(noncanonical_source, 1, 1)
            .expect("resolve normalized original path");
        assert_eq!((generated.line, generated.column), (1, 0));

        #[cfg(windows)]
        {
            let differently_cased_source = workspace
                .root
                .join("SRC")
                .join("MAIN.TS")
                .to_string_lossy()
                .to_uppercase();
            maps.original_to_generated(differently_cased_source, 0, 4)
                .expect("match a case-insensitive Windows path");
        }
    }

    #[test]
    fn lookup_uses_same_line_lub_then_glb_without_crossing_lines() {
        let workspace = TestWorkspace::new();
        workspace.write_valid_map();
        let maps = SourceMaps::from_workspace(&workspace.root).expect("load workspace map");

        let forward_lub = maps
            .generated_to_original("scripts/main.js", 0, 5)
            .expect("use generated LUB");
        assert_eq!((forward_lub.line, forward_lub.column), (0, 10));
        let forward_glb = maps
            .generated_to_original("scripts/main.js", 0, 9)
            .expect("use generated GLB");
        assert_eq!((forward_glb.line, forward_glb.column), (0, 10));
        assert!(matches!(
            maps.generated_to_original("scripts/main.js", 2, 0),
            Err(SourceMapError::NotFound(_))
        ));

        let source = workspace.root.join("src").join("main.ts");
        let reverse_lub = maps
            .original_to_generated(&source, 0, 5)
            .expect("use original LUB");
        assert_eq!((reverse_lub.line, reverse_lub.column), (0, 8));
        let reverse_glb = maps
            .original_to_generated(&source, 0, 11)
            .expect("use original GLB");
        assert_eq!((reverse_glb.line, reverse_glb.column), (0, 8));
        assert!(matches!(
            maps.original_to_generated(source, 2, 0),
            Err(SourceMapError::NotFound(_))
        ));
    }

    #[test]
    fn reports_missing_workspace_map_as_not_found() {
        let workspace = TestWorkspace::new();
        assert!(matches!(
            SourceMaps::from_workspace(&workspace.root),
            Err(SourceMapError::NotFound(_))
        ));
    }

    #[test]
    fn reports_malformed_workspace_map_as_invalid() {
        let workspace = TestWorkspace::new();
        fs::write(workspace.map_path(), "{malformed").expect("write malformed map");
        assert!(matches!(
            SourceMaps::from_workspace(&workspace.root),
            Err(SourceMapError::Invalid(_))
        ));
    }

    #[test]
    fn empty_resolver_remains_compatible() {
        let maps = SourceMaps::new();
        assert!(matches!(
            maps.generated_to_original("/scripts/main.js", 0, 0),
            Err(SourceMapError::NotFound(_))
        ));
    }
}

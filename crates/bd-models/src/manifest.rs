//! The compiled-in model manifest: every model the app knows how to fetch,
//! with pinned sources and the hashes that make a download trustworthy.

use std::collections::HashSet;
use std::sync::LazyLock;

use serde::Deserialize;

/// Source of the built-in manifest, checked into the crate next to `src/`.
const BUILTIN_TOML: &str = include_str!("../manifest.toml");

static BUILTIN: LazyLock<Manifest> = LazyLock::new(|| {
    // The unit tests parse and validate this exact text, so a failure here
    // means the binary was built from an untested manifest.
    Manifest::parse(BUILTIN_TOML).expect("built-in manifest.toml is invalid")
});

/// What a model is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelKind {
    /// Speech-to-text (sherpa-onnx).
    Asr,
    /// Voice activity detection (sherpa-onnx).
    Vad,
    /// Prose cleanup LLM (llama.cpp GGUF).
    Cleanup,
}

/// One file of a model, as it sits in the model's store directory.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelFile {
    /// Relative path, '/'-separated. Also the suffix appended to each source
    /// base URL.
    pub path: String,
    /// Exact size in bytes.
    pub size: u64,
    /// Lowercase hex SHA-256 of the whole file.
    pub sha256: String,
}

/// A model published only as a `.tar.bz2`: the archive is downloaded from
/// the sources like a file, then the model's `files` are taken out of it
/// (each checked against its own hash) and the archive is deleted.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelArchive {
    /// The archive's file name, appended to each source base URL.
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// The folder inside the archive that holds the files, ending in '/'.
    pub strip: String,
}

/// Sampling parameters for a cleanup model, in llama.cpp terms.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sampling {
    /// 0.0 means greedy decoding.
    pub temperature: f32,
    /// 0 disables top-k.
    pub top_k: u32,
    pub top_p: f32,
    pub min_p: f32,
    /// 1.0 disables the repeat penalty.
    pub repeat_penalty: f32,
    pub presence_penalty: f32,
    /// Hard cap on generated tokens for one segment.
    pub max_tokens: u32,
}

/// How a cleanup model is prompted and sampled.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupParams {
    /// Id of the prompt template (system prompt + framing) the cleanup crate
    /// uses with this model.
    pub prompt_template: String,
    /// Whether to let a hybrid reasoning model think before answering.
    #[serde(default)]
    pub thinking: bool,
    pub sampling: Sampling,
}

/// One model in the catalog.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelEntry {
    /// Stable id; also the model's directory name in the store.
    pub id: String,
    pub kind: ModelKind,
    /// Name shown to the user.
    pub name: String,
    /// SPDX license id of the weights.
    pub license: String,
    /// Credit line for the About box, where the license asks for one.
    #[serde(default)]
    pub attribution: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    /// The app can't dictate without it (ASR, VAD).
    #[serde(default)]
    pub required: bool,
    /// False for catalog placeholders whose weights don't exist yet.
    #[serde(default = "default_true")]
    pub available: bool,
    /// The one of its kind "auto" chooses when it's downloaded, and the one
    /// the app suggests downloading. At most one per kind.
    #[serde(default)]
    pub recommended: bool,
    /// Base URLs ending in '/', tried in order.
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub files: Vec<ModelFile>,
    /// Where `files` come from when they aren't published one by one.
    #[serde(default)]
    pub archive: Option<ModelArchive>,
    /// Required for available cleanup models, absent for every other kind.
    #[serde(default)]
    pub cleanup: Option<CleanupParams>,
}

fn default_true() -> bool {
    true
}

impl ModelEntry {
    /// Sum of all file sizes: what the model takes on disk.
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// What a download fetches: the archive, or every file.
    pub fn download_size(&self) -> u64 {
        self.archive.as_ref().map_or_else(|| self.total_size(), |a| a.size)
    }

    /// The manifest entry for `path`, if this model has such a file.
    pub fn file(&self, path: &str) -> Option<&ModelFile> {
        self.files.iter().find(|f| f.path == path)
    }

    /// Download URL of `file` from the source base `source`.
    pub fn file_url(source: &str, file: &ModelFile) -> String {
        format!("{source}{}", file.path)
    }
}

/// The full catalog.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    #[serde(rename = "model")]
    pub models: Vec<ModelEntry>,
}

/// Why a manifest was rejected.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest is not valid TOML for the schema: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("manifest entry {model:?}: {reason}")]
    Invalid { model: String, reason: String },
}

impl Manifest {
    /// The manifest compiled into this build.
    pub fn builtin() -> &'static Manifest {
        &BUILTIN
    }

    /// Parses and validates a manifest.
    pub fn parse(text: &str) -> Result<Manifest, ManifestError> {
        let manifest: Manifest = toml::from_str(text)?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Looks a model up by id.
    pub fn get(&self, id: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|m| m.id == id)
    }

    /// All models of one kind, in manifest order.
    pub fn of_kind(&self, kind: ModelKind) -> impl Iterator<Item = &ModelEntry> {
        self.models.iter().filter(move |m| m.kind == kind)
    }

    /// Checks every invariant the store and downloader rely on.
    pub fn validate(&self) -> Result<(), ManifestError> {
        let mut ids = HashSet::new();
        let mut recommended = HashSet::new();
        for model in &self.models {
            let invalid = |reason: String| ManifestError::Invalid {
                model: model.id.clone(),
                reason,
            };
            if !is_valid_model_id(&model.id) {
                return Err(invalid("id must be 1-128 chars of [a-z0-9._-], not starting with '.'".into()));
            }
            if !ids.insert(model.id.as_str()) {
                return Err(invalid("duplicate id".into()));
            }
            if model.name.trim().is_empty() || model.license.trim().is_empty() {
                return Err(invalid("name and license must be non-empty".into()));
            }
            if model.recommended && (!model.available || !recommended.insert(model.kind)) {
                return Err(invalid("recommended: only one available model per kind".into()));
            }
            validate_payload(model).map_err(invalid)?;
            validate_cleanup(model).map_err(invalid)?;
        }
        Ok(())
    }
}

/// Files and sources: present and well-formed when available, absent when not.
fn validate_payload(model: &ModelEntry) -> Result<(), String> {
    if !model.available {
        if !model.files.is_empty() || !model.sources.is_empty() {
            return Err("an unavailable model must list no files or sources".into());
        }
        if model.required {
            return Err("a required model must be available".into());
        }
        return Ok(());
    }
    if model.files.is_empty() {
        return Err("no files".into());
    }
    if model.sources.is_empty() {
        return Err("no sources".into());
    }
    for source in &model.sources {
        if !source.starts_with("https://") || !source.ends_with('/') {
            return Err(format!("source {source:?} must be an https URL ending in '/'"));
        }
        if source.contains("/resolve/main/") {
            return Err(format!("source {source:?} is not pinned to a revision"));
        }
    }
    if let Some(a) = &model.archive {
        if !a.path.ends_with(".tar.bz2") || !is_valid_file_path(&a.path) || a.size == 0 || !is_sha256_hex(&a.sha256) {
            return Err(format!("archive {:?} must be a .tar.bz2 with a size and sha256", a.path));
        }
        if !a.strip.ends_with('/') || a.strip.contains("..") {
            return Err(format!("archive strip {:?} must be a folder ending in '/'", a.strip));
        }
    }
    let mut paths = HashSet::new();
    for file in &model.files {
        if !is_valid_file_path(&file.path) {
            return Err(format!("file path {:?} is not a safe relative path", file.path));
        }
        if !paths.insert(file.path.as_str()) {
            return Err(format!("duplicate file path {:?}", file.path));
        }
        if file.size == 0 {
            return Err(format!("{}: size must be > 0", file.path));
        }
        if !is_sha256_hex(&file.sha256) {
            return Err(format!("{}: sha256 must be 64 lowercase hex chars", file.path));
        }
    }
    Ok(())
}

fn validate_cleanup(model: &ModelEntry) -> Result<(), String> {
    let Some(params) = &model.cleanup else {
        if model.kind == ModelKind::Cleanup && model.available {
            return Err("an available cleanup model needs [model.cleanup] params".into());
        }
        return Ok(());
    };
    if model.kind != ModelKind::Cleanup {
        return Err("only cleanup models take [model.cleanup] params".into());
    }
    if params.prompt_template.trim().is_empty() {
        return Err("prompt_template must be non-empty".into());
    }
    let s = &params.sampling;
    let in_unit = |v: f32| (0.0..=1.0).contains(&v);
    if !(s.temperature >= 0.0 && s.temperature.is_finite()) {
        return Err("temperature must be finite and >= 0".into());
    }
    if !(s.top_p > 0.0 && s.top_p <= 1.0) || !in_unit(s.min_p) {
        return Err("top_p must be in (0, 1] and min_p in [0, 1]".into());
    }
    if !(s.repeat_penalty > 0.0 && s.repeat_penalty.is_finite()) || !s.presence_penalty.is_finite() {
        return Err("repeat_penalty must be > 0 and presence_penalty finite".into());
    }
    if s.max_tokens == 0 {
        return Err("max_tokens must be > 0".into());
    }
    Ok(())
}

/// Model ids name directories, so they're restricted to a portable alphabet.
pub(crate) fn is_valid_model_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
}

/// A '/'-separated relative path whose components are portable file names,
/// safe to join onto both a directory and a URL. Hidden names and `.part`
/// suffixes are reserved for the store's own bookkeeping.
pub(crate) fn is_valid_file_path(path: &str) -> bool {
    !path.ends_with(crate::store::PART_SUFFIX)
        && path.split('/').all(|c| {
            !c.is_empty()
                && !c.starts_with('.')
                && c.len() <= 255
                && c.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        })
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_parses_and_validates() {
        let m = Manifest::parse(BUILTIN_TOML).unwrap();
        assert!(std::ptr::eq(Manifest::builtin(), Manifest::builtin()));
        assert_eq!(m, *Manifest::builtin());
        for model in &m.models {
            if model.available {
                assert!(model.sources.iter().any(|s| s.starts_with("https://")));
                assert!(model.total_size() > 0);
            }
        }
    }

    #[test]
    fn only_one_recommended_model_per_kind() {
        let two = format!("{GOOD}{}", GOOD.replace("vad-x", "vad-y"));
        assert!(Manifest::parse(&two.replace("license = \"MIT\"", "license = \"MIT\"\n        recommended = true")).is_err());
        assert!(Manifest::parse(&two).is_ok());
        let builtin = Manifest::builtin();
        assert_eq!(builtin.of_kind(ModelKind::Cleanup).filter(|m| m.recommended).count(), 1);
    }

    #[test]
    fn builtin_has_required_models_of_each_kind() {
        let m = Manifest::builtin();
        for kind in [ModelKind::Asr, ModelKind::Vad] {
            assert!(m.of_kind(kind).any(|e| e.required && e.available), "{kind:?}");
        }
        let cleanup: Vec<_> = m.of_kind(ModelKind::Cleanup).collect();
        assert!(cleanup.iter().any(|e| e.available && e.cleanup.is_some()));
        let asr = m.get("asr-parakeet-unified-en-0.6b-int8").unwrap();
        assert!(asr.required);
        assert_eq!(asr.files.len(), 4);
        assert!(asr.file("tokens.txt").is_some());
    }

    const GOOD: &str = r#"
        [[model]]
        id = "vad-x"
        kind = "vad"
        name = "X"
        license = "MIT"
        sources = ["https://example.com/r/abc/"]
        files = [{ path = "x.onnx", size = 3, sha256 = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad" }]
    "#;

    fn rejects(text: &str) -> String {
        match Manifest::parse(text) {
            Err(ManifestError::Invalid { reason, .. }) => reason,
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn minimal_entry_is_accepted() {
        let m = Manifest::parse(GOOD).unwrap();
        assert!(m.models[0].available);
        assert_eq!(
            ModelEntry::file_url(&m.models[0].sources[0], &m.models[0].files[0]),
            "https://example.com/r/abc/x.onnx"
        );
    }

    #[test]
    fn rejects_duplicate_ids() {
        let text = format!("{GOOD}\n{}", GOOD.replace("[[model]]", "\n[[model]]"));
        assert_eq!(rejects(&text), "duplicate id");
    }

    #[test]
    fn rejects_bad_hashes_sizes_and_sources() {
        assert!(rejects(&GOOD.replace("ba7816", "BA7816")).contains("sha256"));
        assert!(rejects(&GOOD.replace("15ad\"", "15a\"")).contains("sha256"));
        assert!(rejects(&GOOD.replace("size = 3", "size = 0")).contains("size"));
        assert!(rejects(&GOOD.replace("https://example.com/r/abc/", "http://example.com/")).contains("https"));
        assert!(rejects(&GOOD.replace("/r/abc/", "/r/abc")).contains("https"));
        assert!(rejects(&GOOD.replace("/r/abc/", "/x/resolve/main/")).contains("pinned"));
        assert_eq!(rejects(&GOOD.replace("sources = [\"https://example.com/r/abc/\"]", "sources = []")), "no sources");
    }

    #[test]
    fn rejects_unsafe_ids_and_paths() {
        assert!(rejects(&GOOD.replace("vad-x", "../x")).contains("id"));
        assert!(rejects(&GOOD.replace("vad-x", "Vad")).contains("id"));
        for bad in ["../x.onnx", "/x.onnx", ".hidden", "a//b", "x.onnx.part", "a\\\\b", "sp ace"] {
            let reason = rejects(&GOOD.replace("\"x.onnx\"", &format!("\"{bad}\"")));
            assert!(reason.contains("safe relative path"), "{bad}: {reason}");
        }
        assert!(is_valid_file_path("sub/dir/x.onnx"));
    }

    #[test]
    fn unavailable_entries_carry_no_payload() {
        let placeholder = "[[model]]\nid = \"c\"\nkind = \"cleanup\"\nname = \"C\"\nlicense = \"MIT\"\navailable = false\n";
        assert!(Manifest::parse(placeholder).is_ok());
        let with_files = GOOD.replace("kind = \"vad\"", "kind = \"vad\"\navailable = false");
        assert!(rejects(&with_files).contains("unavailable"));
    }

    #[test]
    fn cleanup_params_are_required_and_checked() {
        let cleanup = GOOD.replace("kind = \"vad\"", "kind = \"cleanup\"");
        assert!(rejects(&cleanup).contains("needs [model.cleanup]"));
        let params = "\n[model.cleanup]\nprompt_template = \"t\"\n[model.cleanup.sampling]\n\
            temperature = 0.0\ntop_k = 0\ntop_p = 1.0\nmin_p = 0.0\nrepeat_penalty = 1.0\n\
            presence_penalty = 0.0\nmax_tokens = 64\n";
        let ok = format!("{cleanup}{params}");
        assert!(!Manifest::parse(&ok).unwrap().models[0].cleanup.as_ref().unwrap().thinking);
        assert!(rejects(&ok.replace("top_p = 1.0", "top_p = 0.0")).contains("top_p"));
        assert!(rejects(&ok.replace("max_tokens = 64", "max_tokens = 0")).contains("max_tokens"));
        assert!(rejects(&format!("{GOOD}{params}")).contains("only cleanup"));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        assert!(matches!(
            Manifest::parse(&GOOD.replace("license = \"MIT\"", "license = \"MIT\"\nlicence = \"x\"")),
            Err(ManifestError::Parse(_))
        ));
    }
}

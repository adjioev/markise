//! Size limits of the data a package may hold (ADR 0019). A package over a limit is refused;
//! nothing is truncated.

/// One JSON file: the manifest, a schema, the palette or the views.
pub const MAX_JSON_BYTES: usize = 1 << 20;
/// Blocks in a package.
pub const MAX_BLOCKS: usize = 512;
/// Fields in one block's schema.
pub const MAX_FIELDS: usize = 256;
/// Palette groups.
pub const MAX_GROUPS: usize = 256;
/// Palette presets.
pub const MAX_PRESETS: usize = 2048;
/// A library, block, field, preset, group, icon or accent name, in bytes.
pub const MAX_NAME_BYTES: usize = 128;
/// A label, description or keyword text, in bytes.
pub const MAX_TEXT_BYTES: usize = 4096;
/// The Markdown of one preset, in bytes.
pub const MAX_PRESET_MARKDOWN_BYTES: usize = 64 << 10;
/// Validation issues reported for one package; the rest are dropped.
pub const MAX_ISSUES: usize = 32;

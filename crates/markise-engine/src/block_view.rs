//! A package's blocks as the view knows them (ADR 0021): [`markise_layout::BlockViews`] from a
//! package's views and schemas and the field model. Which fields a block has and whether it fits
//! them is the same answer the form gets. The view asks once per version of a block on screen.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use markise_layout::{BlockLook, BlockViews, NativeBlock, NativeKind, NativeRole, NativeView};
use markise_md::ParseOptions;
use markise_package::{PackageIndex, ViewKind};
use schema_form_core::{Control, FormSpec, Selection};

use crate::block_fields::{BlockFields, BlockFit, FieldDecl, FieldKind, decls_from_spec};

/// What a block of one name and variant takes, read from the package once.
struct Variant {
    spec: FormSpec,
    decls: Vec<FieldDecl>,
    /// `None`: the block has a schema and no view.
    view: Option<Arc<NativeView>>,
}

/// By block name and variant.
type Variants = HashMap<(String, Option<String>), Option<Arc<Variant>>>;

/// The views of one package.
pub struct PackageViews {
    index: Arc<PackageIndex>,
    language: String,
    id: u64,
    variants: Mutex<Variants>,
}

fn kind_of(k: ViewKind) -> NativeKind {
    match k {
        ViewKind::Callout => NativeKind::Callout,
        ViewKind::Card => NativeKind::Card,
        ViewKind::Choices => NativeKind::Choices,
        ViewKind::Media => NativeKind::Media,
    }
}

fn role_of(k: FieldKind) -> NativeRole {
    match k {
        FieldKind::Heading { .. } => NativeRole::Title,
        FieldKind::Text => NativeRole::Text,
        FieldKind::Quote => NativeRole::Quote,
        FieldKind::List => NativeRole::ListItem,
        FieldKind::Choices => NativeRole::Answer,
        FieldKind::Image => NativeRole::Image,
    }
}

impl PackageViews {
    pub fn new(index: Arc<PackageIndex>, language: &str) -> PackageViews {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (Arc::as_ptr(&index) as usize, language).hash(&mut h);
        PackageViews {
            index,
            language: language.to_string(),
            id: h.finish(),
            variants: Mutex::default(),
        }
    }

    fn read_variant(&self, name: &str, variant: Option<&str>) -> Option<Variant> {
        let entry = self.index.block(name)?;
        let input = self.index.form_input(name, &self.language)?.ok()?;
        let spec = FormSpec::parse(&input.schema, &input.hints, variant).ok()?;
        let decls = decls_from_spec(&spec).ok()?;
        let single_answer = spec.fields.iter().any(|f| {
            matches!(
                f.control,
                Control::Choices {
                    selection: Selection::Single,
                    ..
                }
            )
        });
        let block = (entry.schema.label.as_ref()).map_or(name, |l| l.get(&self.language));
        let label = match variant.and_then(|v| entry.schema.variant_label(v, &self.language)) {
            Some(v) if !v.is_empty() => format!("{block} · {v}"),
            _ => block.to_string(),
        };
        // a name; a package gives it meaning with a file of that name, or leaves it a name
        let icon =
            (entry.view.and_then(|c| c.icon.as_deref())).or_else(|| entry.palette.and_then(|p| p.icon.as_deref()));
        let view = entry.view.map(|config| {
            Arc::new(NativeView {
                kind: kind_of(config.kind),
                accent: config.accent.as_deref().map(Into::into),
                icon: icon.map(|n| format!("assets/{n}.svg").into()),
                label: label.into(),
                single_answer,
            })
        });
        Some(Variant { spec, decls, view })
    }

    /// The block's variant for these arguments: `Err` when they name none, in a block that has some.
    fn variant_of(&self, name: &str, args: &str) -> Result<Option<Arc<Variant>>, ()> {
        let Some(schema) = self.index.block(name).map(|e| e.schema) else {
            return Ok(None);
        };
        let named = schema.variants().any(|v| v == args);
        if !args.is_empty() && !named && schema.variants().next().is_some() {
            return Err(());
        }
        let variant = named.then(|| args.to_string()).filter(|a| !a.is_empty());
        let mut known = self.variants.lock().unwrap_or_else(|e| e.into_inner());
        let found = known
            .entry((name.to_string(), variant.clone()))
            .or_insert_with(|| self.read_variant(name, variant.as_deref()).map(Arc::new));
        Ok(found.clone())
    }
}

/// A block's misfit in words: a field by its label, and an item by its place in the field.
fn reasons(spec: &FormSpec, fit: &BlockFit, source: &str) -> Vec<String> {
    let field_error = |e: &schema_form_core::FieldError| {
        let mut path = e.path.split('/');
        let key = path.next().unwrap_or("");
        let label = spec
            .fields
            .iter()
            .find(|f| f.key == key)
            .map_or(key, |f| f.label.as_str());
        match path.next().and_then(|n| n.parse::<usize>().ok()) {
            Some(n) => format!("{label} {}: {}", n + 1, e.kind),
            None => format!("{label}: {}", e.kind),
        }
    };
    let extra = |r: &std::ops::Range<usize>| {
        let piece = source.get(r.clone()).unwrap_or("");
        let line = piece.lines().next().unwrap_or("");
        let mut shown: String = line.chars().take(40).collect();
        if shown.len() < piece.len() {
            shown.push('…');
        }
        format!("In no field: {shown}")
    };
    (fit.errors.iter().map(field_error))
        .chain(fit.extras.iter().map(extra))
        .collect()
}

impl PackageViews {
    /// The field model of a block with a view, for editing it in place.
    pub fn fields_of(&self, name: &str, args: &str, source: &str, base: usize) -> Option<BlockFields> {
        let variant = self.variant_of(name, args).ok()??;
        variant.view.as_ref()?;
        BlockFields::decode(source, base, &variant.decls, &ParseOptions::default()).ok()
    }
}

impl BlockViews for PackageViews {
    fn id(&self) -> u64 {
        self.id
    }

    fn has_view(&self, name: &str) -> bool {
        self.index.block(name).is_some_and(|e| e.view.is_some())
    }

    fn has_schema(&self, name: &str) -> bool {
        self.index.block(name).is_some()
    }

    fn look(&self, name: &str, args: &str, source: &str) -> BlockLook {
        let variant = match self.variant_of(name, args) {
            Ok(Some(variant)) => variant,
            // a package saying what a form cannot honour is the panel's to explain
            Ok(None) => return BlockLook::Generic,
            Err(()) => {
                return BlockLook::Outside {
                    reasons: vec![format!("The block has no variant `{args}`.")],
                    native: None,
                };
            }
        };
        let Ok(fields) = BlockFields::decode(source, 0, &variant.decls, &ParseOptions::default()) else {
            return BlockLook::Generic;
        };
        let native = variant.view.as_ref().map(|view| NativeBlock {
            view: view.clone(),
            parts: (fields.field_regions().into_iter())
                .map(|(r, kind)| (r.start as u32..r.end as u32, role_of(kind)))
                .collect(),
        });
        let fit = fields.check(&variant.spec);
        if !fit.fits() {
            return BlockLook::Outside {
                reasons: reasons(&variant.spec, &fit, source),
                native,
            };
        }
        native.map_or(BlockLook::Generic, BlockLook::Native)
    }
}

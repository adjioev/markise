//! A package's blocks as the view knows them (ADR 0021): [`markise_layout::BlockViews`] from a
//! package's views and schemas and the field model. Which fields a block has and whether it fits
//! them is the same answer the form gets. The view asks once per version of a block on screen.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use markise_layout::{
    BlockLook, BlockViews, NativeBlock, NativeKind, NativeRole, NativeView, PracticeAnswer, PracticeParts,
};
use markise_md::ParseOptions;
use markise_package::{BlockSchema, PackageIndex, ViewKind};
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
    /// The blocks whose presence opens a document in Read.
    default_names: Vec<String>,
}

fn kind_of(k: ViewKind) -> NativeKind {
    match k {
        ViewKind::Callout => NativeKind::Callout,
        ViewKind::Card => NativeKind::Card,
        ViewKind::Choices => NativeKind::Choices,
        ViewKind::Media => NativeKind::Media,
    }
}

/// The form of block `name` for `variant` and where its fields live, or why a form cannot be made,
/// in the words the panel shows.
pub fn read_form(
    index: &PackageIndex,
    name: &str,
    language: &str,
    variant: Option<&str>,
) -> Result<(FormSpec, Vec<FieldDecl>), String> {
    let input = (index.form_input(name, language))
        .ok_or("the package has no schema for it")?
        .map_err(|e| e.to_string())?;
    let spec = FormSpec::parse(&input.schema, &input.hints, variant).map_err(|e| e.to_string())?;
    let decls = decls_from_spec(&spec).map_err(|f| format!("field `{f}` has no place in the block"))?;
    Ok((spec, decls))
}

/// One reason a block does not fit, as its `⚠` words it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockReason {
    pub text: String,
    /// The piece of the block it is about, in source coordinates of the block.
    pub range: Option<std::ops::Range<usize>>,
}

/// What is wrong with a block, the answer the view gets in other terms ([`BlockLook`]) and what
/// `markise-cli blocks check` reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockVerdict {
    Fits,
    /// The package has no block of this name.
    NoSchema,
    /// The package says something a form cannot honour about it; the reason the panel shows.
    NoForm(String),
    /// Not filled in yet: the block keeps its native view.
    Unfilled(Vec<BlockReason>),
    /// Outside its schema: a card.
    Outside(Vec<BlockReason>),
}

/// The block's variant for these arguments: `Err` when they name none, in a block that has some.
fn named_variant(schema: &BlockSchema, args: &str) -> Result<Option<String>, ()> {
    let named = schema.variants().any(|v| v == args);
    if !args.is_empty() && !named && schema.variants().next().is_some() {
        return Err(());
    }
    Ok(named.then(|| args.to_string()).filter(|a| !a.is_empty()))
}

fn no_variant(args: &str) -> BlockReason {
    BlockReason {
        text: format!("The block has no variant `{args}`."),
        range: None,
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
        // has no region of the block, so nothing asks
        FieldKind::Argument => NativeRole::Text,
    }
}

impl PackageViews {
    pub fn new(index: Arc<PackageIndex>, language: &str) -> PackageViews {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (Arc::as_ptr(&index) as usize, language).hash(&mut h);
        let default_names = (index.block_names())
            .filter(|n| {
                (index.block(n)).is_some_and(|e| e.view.and_then(|v| v.practice.as_ref()).is_some_and(|p| p.default))
            })
            .map(str::to_string)
            .collect();
        PackageViews {
            default_names,
            index,
            language: language.to_string(),
            id: h.finish(),
            variants: Mutex::default(),
        }
    }

    fn read_variant(&self, name: &str, variant: Option<&str>) -> Option<Variant> {
        let entry = self.index.block(name)?;
        let (spec, decls) = read_form(&self.index, name, &self.language, variant).ok()?;
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
                fill: config.fill,
                label_row: config.label,
            })
        });
        Some(Variant { spec, decls, view })
    }

    /// The block's variant for these arguments: `Err` when they name none, in a block that has some.
    fn variant_of(&self, name: &str, args: &str) -> Result<Option<Arc<Variant>>, ()> {
        let Some(schema) = self.index.block(name).map(|e| e.schema) else {
            return Ok(None);
        };
        let variant = named_variant(schema, args)?;
        let mut known = self.variants.lock().unwrap_or_else(|e| e.into_inner());
        let found = known
            .entry((name.to_string(), variant.clone()))
            .or_insert_with(|| self.read_variant(name, variant.as_deref()).map(Arc::new));
        Ok(found.clone())
    }
}

/// A block's misfit in words: a field by its label, and an item by its place in the field.
fn reasons(spec: &FormSpec, fit: &BlockFit, source: &str) -> Vec<BlockReason> {
    let field_error = |e: &schema_form_core::FieldError| {
        let mut path = e.path.split('/');
        let key = path.next().unwrap_or("");
        let label = spec
            .fields
            .iter()
            .find(|f| f.key == key)
            .map_or(key, |f| f.label.as_str());
        let text = match path.next().and_then(|n| n.parse::<usize>().ok()) {
            Some(n) => format!("{label} {}: {}", n + 1, e.kind),
            None => format!("{label}: {}", e.kind),
        };
        BlockReason { text, range: None }
    };
    let extra = |r: &std::ops::Range<usize>| {
        let piece = source.get(r.clone()).unwrap_or("");
        let line = piece.lines().next().unwrap_or("");
        let mut shown: String = line.chars().take(40).collect();
        if shown.len() < piece.len() {
            shown.push('…');
        }
        BlockReason {
            text: format!("In no field: {shown}"),
            range: Some(r.clone()),
        }
    };
    (fit.errors.iter().map(field_error))
        .chain(fit.extras.iter().map(extra))
        .collect()
}

/// `Some` when the block does not fit: whether it is only yet to be filled in, and why. Only a block
/// with a view has a way of showing that, so one with none is outside its schema (ADR 0021).
fn misfit(variant: &Variant, fields: &BlockFields, source: &str) -> Option<(bool, Vec<BlockReason>)> {
    let fit = fields.check(&variant.spec);
    (!fit.fits()).then(|| {
        let unfilled = variant.view.is_some() && fit.only_unfilled(&fields.values());
        (unfilled, reasons(&variant.spec, &fit, source))
    })
}

fn texts(reasons: Vec<BlockReason>) -> Vec<String> {
    reasons.into_iter().map(|r| r.text).collect()
}

impl PackageViews {
    /// What is wrong with a block, in the words of its `⚠`, or that the package does not know it.
    pub fn verdict(&self, name: &str, args: &str, source: &str) -> BlockVerdict {
        let Some(entry) = self.index.block(name) else {
            return BlockVerdict::NoSchema;
        };
        let variant = match self.variant_of(name, args) {
            Ok(Some(variant)) => variant,
            Ok(None) => {
                let wanted = named_variant(entry.schema, args).ok().flatten();
                let why = read_form(&self.index, name, &self.language, wanted.as_deref());
                return BlockVerdict::NoForm(why.err().unwrap_or_default());
            }
            Err(()) => return BlockVerdict::Outside(vec![no_variant(args)]),
        };
        let fields = match BlockFields::decode(source, 0, &variant.decls, &ParseOptions::default()) {
            Ok(fields) => fields,
            Err(e) => return BlockVerdict::NoForm(e.to_string()),
        };
        match misfit(&variant, &fields, source) {
            None => BlockVerdict::Fits,
            Some((true, reasons)) => BlockVerdict::Unfilled(reasons),
            Some((false, reasons)) => BlockVerdict::Outside(reasons),
        }
    }
}

impl PackageViews {
    /// The field model of a block with a view, for editing it in place.
    pub fn fields_of(&self, name: &str, args: &str, source: &str, base: usize) -> Option<BlockFields> {
        let variant = self.variant_of(name, args).ok()??;
        variant.view.as_ref()?;
        BlockFields::decode(source, base, &variant.decls, &ParseOptions::default()).ok()
    }
}

impl PackageViews {
    /// The field model and form of a block, with a view or without: what its markup comes from.
    pub fn block_model(&self, name: &str, args: &str, source: &str) -> Option<(BlockFields, FormSpec)> {
        let variant = self.variant_of(name, args).ok()??;
        let fields = BlockFields::decode(source, 0, &variant.decls, &ParseOptions::default()).ok()?;
        Some((fields, variant.spec.clone()))
    }
}

impl PackageViews {
    /// The blocks whose presence opens a document in Read.
    pub fn read_by_default(&self) -> &[String] {
        &self.default_names
    }

    /// Whether any block of the package can be practised.
    pub fn any_practice(&self) -> bool {
        (self.index.block_names()).any(|n| {
            self.index
                .block(n)
                .is_some_and(|e| e.view.is_some_and(|v| v.practice.is_some()))
        })
    }

    /// What Practice needs of a block, when its view can be practised. Read for what the view holds and
    /// no further, so that a block with no practice pays nothing.
    fn practice_parts(&self, name: &str, fields: &BlockFields) -> Option<PracticeParts> {
        let practice = self.index.block(name)?.view?.practice.as_ref()?;
        let answers = (fields.slots().into_iter())
            .filter(|s| s.kind == FieldKind::Choices)
            .map(|s| {
                let correct = fields.slot_is_marked(s.at);
                let r = s.range();
                PracticeAnswer {
                    range: r.start as u32..r.end as u32,
                    correct,
                    text: s.value().into(),
                }
            })
            .collect();
        let reveal = (practice.reveal.iter())
            .flat_map(|field| fields.regions_of(field))
            .map(|r| r.start as u32..r.end as u32)
            .collect();
        Some(PracticeParts {
            has_choices: fields.fields().iter().any(|f| f.kind == FieldKind::Choices),
            answers,
            reveal,
        })
    }
}

impl BlockViews for PackageViews {
    fn has_practice(&self, name: &str) -> bool {
        self.index
            .block(name)
            .is_some_and(|e| e.view.is_some_and(|v| v.practice.is_some()))
    }

    fn any_practice(&self) -> bool {
        PackageViews::any_practice(self)
    }

    fn practice_hides_quotes(&self, name: &str) -> bool {
        self.index.block(name).is_some_and(|e| {
            let reveal = e.view.and_then(|v| v.practice.as_ref()).map_or(&[][..], |p| &p.reveal);
            (e.schema.fields.iter()).any(|f| f.kind == markise_package::FieldKind::Quote && reveal.contains(&f.name))
        })
    }

    fn id(&self) -> u64 {
        self.id
    }

    fn has_view(&self, name: &str) -> bool {
        self.index.block(name).is_some_and(|e| e.view.is_some())
    }

    fn has_schema(&self, name: &str) -> bool {
        self.index.block(name).is_some()
    }

    fn label_row(&self, name: &str) -> bool {
        (self.index.block(name).and_then(|e| e.view)).is_none_or(|c| c.label)
    }

    fn look(&self, name: &str, args: &str, source: &str) -> BlockLook {
        let variant = match self.variant_of(name, args) {
            Ok(Some(variant)) => variant,
            // a package saying what a form cannot honour is the panel's to explain
            Ok(None) => return BlockLook::Generic,
            Err(()) => {
                return BlockLook::Outside {
                    reasons: texts(vec![no_variant(args)]),
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
            practice: self.practice_parts(name, &fields),
        });
        match (misfit(&variant, &fields, source), native) {
            (None, native) => native.map_or(BlockLook::Generic, BlockLook::Native),
            (Some((true, reasons)), Some(block)) => BlockLook::Unfilled {
                reasons: texts(reasons),
                block,
            },
            (Some((_, reasons)), native) => BlockLook::Outside {
                reasons: texts(reasons),
                native,
            },
        }
    }
}

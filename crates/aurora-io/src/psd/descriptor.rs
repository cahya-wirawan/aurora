//! Photoshop's action descriptor format (0.180.0), the encoding of the
//! object-based effects blocks (`lfx2`, `lfxs`, `lmfx`) and of many other
//! tagged blocks: a small, owned [`Value`] tree.
//!
//! **Bounded against hostile input**, since every byte comes from the
//! file:
//!
//! - nesting at most [`MAX_DEPTH`] containers deep (the parser recurses,
//!   so this also bounds its stack);
//! - every count is checked against the bytes left times the smallest
//!   encoding of one item *before* anything is allocated, so a count of
//!   `0xFFFF_FFFF` in an 8-byte block fails at once;
//! - every value parsed is charged to an [`ItemBudget`] shared by the
//!   whole file ([`MAX_FILE_ITEMS`]), the way `vector::Budget` charges
//!   vector-mask work;
//! - string and key lengths are checked against the bytes left.
//!
//! An unknown `OSType` or a truncated value fails the block being parsed
//! with a [`DescriptorError`]; the caller reports that layer's effects as
//! unreadable. Nothing here panics or fails the file.
//!
//! The format follows psd-tools 1.17.4 (`psd/descriptor.py`).

/// The deepest container nesting accepted.
pub(super) const MAX_DEPTH: usize = 16;

/// The most values all the descriptors of one file may hold together.
pub(super) const MAX_FILE_ITEMS: u64 = 1_000_000;

/// The longest key or class id accepted, in bytes. Real keys are 4-byte
/// codes or short names (`masterFXSwitch`, `dropShadowMulti`).
const MAX_KEY_BYTES: usize = 1024;

/// The most items a container reserves room for up front. Counts are
/// already checked against the bytes left; this second bound keeps a
/// container's first allocation small whatever that check allows.
const PREALLOC_CAP: usize = 1024;

/// What one file's descriptors may still parse, in values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ItemBudget {
    pub remaining: u64,
}

impl ItemBudget {
    pub(super) fn new() -> Self {
        Self {
            remaining: MAX_FILE_ITEMS,
        }
    }

    fn charge(&mut self) -> Result<(), DescriptorError> {
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or(DescriptorError::BudgetSpent)?;
        Ok(())
    }
}

/// Why a descriptor block could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DescriptorError {
    Truncated,
    UnknownType([u8; 4]),
    TooDeep,
    /// A count or length larger than the bytes left could hold.
    CountTooLarge,
    BadString,
    BudgetSpent,
    /// A block header this parser does not know (descriptor version
    /// other than 16).
    UnknownVersion,
}

/// A descriptor: its class id and its items, in file order.
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct Descriptor {
    pub class: Vec<u8>,
    pub items: Vec<(Vec<u8>, Value)>,
}

impl Descriptor {
    /// The first item named `key`.
    pub(super) fn get(&self, key: &[u8]) -> Option<&Value> {
        self.items
            .iter()
            .find(|(k, _)| k.as_slice() == key)
            .map(|(_, v)| v)
    }
}

/// One descriptor value.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Value {
    /// `Objc`, `GlbO` and `ObAr`.
    Descriptor(Descriptor),
    /// `VlLs`.
    List(Vec<Value>),
    /// `doub`.
    Double(f64),
    /// `UntF`: a unit code (`#Prc`, `#Ang`, `#Pxl`, …) and a value.
    Unit { unit: [u8; 4], value: f64 },
    /// `UnFl`.
    Units { unit: [u8; 4], values: Vec<f64> },
    /// `long`.
    Integer(i32),
    /// `comp`.
    LargeInteger(i64),
    /// `bool`.
    Bool(bool),
    /// `enum`: type id and value.
    Enum { type_id: Vec<u8>, value: Vec<u8> },
    /// `TEXT`.
    Text(String),
    /// `type`/`GlbC`.
    Class { name: String, class: Vec<u8> },
    /// `obj `: the kinds of its reference items, in order (their
    /// contents are parsed and checked, not kept — nothing uses them).
    Reference(Vec<[u8; 4]>),
    /// `alis`, `tdta`, `Pth `.
    Raw(Vec<u8>),
}

impl Value {
    /// A number, from `doub`, `UntF`, `long` or `comp`.
    pub(super) fn number(&self) -> Option<f64> {
        match self {
            Self::Double(v) | Self::Unit { value: v, .. } => Some(*v),
            Self::Integer(v) => Some(f64::from(*v)),
            #[allow(clippy::cast_precision_loss)] // a descriptor number
            Self::LargeInteger(v) => Some(*v as f64),
            _ => None,
        }
    }

    pub(super) fn bool(&self) -> Option<bool> {
        match self {
            Self::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub(super) fn descriptor(&self) -> Option<&Descriptor> {
        match self {
            Self::Descriptor(d) => Some(d),
            _ => None,
        }
    }

    pub(super) fn list(&self) -> Option<&[Value]> {
        match self {
            Self::List(v) => Some(v),
            _ => None,
        }
    }

    /// A `TEXT` value.
    pub(super) fn text(&self) -> Option<&str> {
        match self {
            Self::Text(v) => Some(v),
            _ => None,
        }
    }

    /// An `enum`'s value.
    pub(super) fn enum_value(&self) -> Option<&[u8]> {
        match self {
            Self::Enum { value, .. } => Some(value),
            _ => None,
        }
    }
}

/// A bounds-checked cursor over one block.
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DescriptorError> {
        let end = self.pos.checked_add(n).ok_or(DescriptorError::Truncated)?;
        let bytes = self
            .data
            .get(self.pos..end)
            .ok_or(DescriptorError::Truncated)?;
        self.pos = end;
        Ok(bytes)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], DescriptorError> {
        let mut out = [0; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn u32(&mut self) -> Result<u32, DescriptorError> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    /// A `u32` count of items each at least `min_size` bytes long,
    /// refused when the bytes left could not hold them.
    fn count(&mut self, min_size: usize) -> Result<usize, DescriptorError> {
        let n = usize::try_from(self.u32()?).map_err(|_| DescriptorError::CountTooLarge)?;
        let need = n
            .checked_mul(min_size)
            .ok_or(DescriptorError::CountTooLarge)?;
        if need > self.remaining() {
            return Err(DescriptorError::CountTooLarge);
        }
        Ok(n)
    }

    /// A key or class id: a `u32` length, `0` meaning a 4-byte code.
    fn key(&mut self) -> Result<Vec<u8>, DescriptorError> {
        let len = usize::try_from(self.u32()?).map_err(|_| DescriptorError::CountTooLarge)?;
        let len = if len == 0 { 4 } else { len };
        if len > MAX_KEY_BYTES || len > self.remaining() {
            return Err(DescriptorError::CountTooLarge);
        }
        Ok(self.take(len)?.to_vec())
    }

    /// A Unicode string: a `u32` count of UTF-16BE code units.
    fn unicode(&mut self) -> Result<String, DescriptorError> {
        let units = self.count(2)?;
        let bytes = self.take(units * 2)?;
        let code: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| {
                u16::from_be_bytes([
                    c.first().copied().unwrap_or(0),
                    c.get(1).copied().unwrap_or(0),
                ])
            })
            .collect();
        String::from_utf16(&code).map_err(|_| DescriptorError::BadString)
    }

    fn f64(&mut self) -> Result<f64, DescriptorError> {
        Ok(f64::from_be_bytes(self.array()?))
    }
}

/// Smallest encoded descriptor item: a key (`u32` length plus at least
/// one byte), the `OSType`, and a one-byte value.
const MIN_ITEM_BYTES: usize = 4 + 1 + 4 + 1;

/// Smallest encoded list item: the `OSType` and a one-byte value.
const MIN_LIST_ITEM_BYTES: usize = 4 + 1;

/// Smallest encoded reference item: its kind plus a 4-byte value.
const MIN_REFERENCE_ITEM_BYTES: usize = 4 + 4;

/// Parses a descriptor body (name, class id, items).
fn descriptor_body(
    c: &mut Cursor<'_>,
    depth: usize,
    budget: &mut ItemBudget,
) -> Result<Descriptor, DescriptorError> {
    if depth > MAX_DEPTH {
        return Err(DescriptorError::TooDeep);
    }
    let _name = c.unicode()?;
    let class = c.key()?;
    let count = c.count(MIN_ITEM_BYTES)?;
    let mut items = Vec::with_capacity(count.min(PREALLOC_CAP));
    for _ in 0..count {
        let key = c.key()?;
        let ostype = c.array::<4>()?;
        let value = value(c, ostype, depth, budget)?;
        items.push((key, value));
    }
    Ok(Descriptor { class, items })
}

/// One value of type `ostype`, at container depth `depth`.
fn value(
    c: &mut Cursor<'_>,
    ostype: [u8; 4],
    depth: usize,
    budget: &mut ItemBudget,
) -> Result<Value, DescriptorError> {
    budget.charge()?;
    Ok(match &ostype {
        b"Objc" | b"GlbO" => Value::Descriptor(descriptor_body(c, depth + 1, budget)?),
        b"ObAr" => {
            let _items_count = c.u32()?;
            Value::Descriptor(descriptor_body(c, depth + 1, budget)?)
        }
        b"VlLs" => {
            if depth + 1 > MAX_DEPTH {
                return Err(DescriptorError::TooDeep);
            }
            let count = c.count(MIN_LIST_ITEM_BYTES)?;
            let mut items = Vec::with_capacity(count.min(PREALLOC_CAP));
            for _ in 0..count {
                let ostype = c.array::<4>()?;
                items.push(value(c, ostype, depth + 1, budget)?);
            }
            Value::List(items)
        }
        b"doub" => Value::Double(c.f64()?),
        b"UntF" => Value::Unit {
            unit: c.array()?,
            value: c.f64()?,
        },
        b"UnFl" => {
            let unit = c.array()?;
            let count = c.count(8)?;
            let mut values = Vec::with_capacity(count.min(PREALLOC_CAP));
            for _ in 0..count {
                values.push(c.f64()?);
            }
            Value::Units { unit, values }
        }
        b"long" => Value::Integer(i32::from_be_bytes(c.array()?)),
        b"comp" => Value::LargeInteger(i64::from_be_bytes(c.array()?)),
        b"bool" => Value::Bool(c.array::<1>()? != [0]),
        b"enum" => Value::Enum {
            type_id: c.key()?,
            value: c.key()?,
        },
        b"TEXT" => Value::Text(c.unicode()?),
        b"type" | b"GlbC" => Value::Class {
            name: c.unicode()?,
            class: c.key()?,
        },
        b"obj " => {
            let count = c.count(MIN_REFERENCE_ITEM_BYTES)?;
            let mut kinds = Vec::with_capacity(count.min(PREALLOC_CAP));
            for _ in 0..count {
                budget.charge()?;
                let kind = c.array::<4>()?;
                reference_item(c, kind)?;
                kinds.push(kind);
            }
            Value::Reference(kinds)
        }
        b"alis" | b"tdta" | b"Pth " => {
            let len = c.count(1)?;
            Value::Raw(c.take(len)?.to_vec())
        }
        _ => return Err(DescriptorError::UnknownType(ostype)),
    })
}

/// Skips one reference item of `kind` (psd-tools' `prop`, `Clss`,
/// `Enmr`, `rele`, `Idnt`, `indx`, `name`).
fn reference_item(c: &mut Cursor<'_>, kind: [u8; 4]) -> Result<(), DescriptorError> {
    match &kind {
        b"prop" => {
            c.unicode()?;
            c.key()?;
            c.key()?;
        }
        b"Clss" => {
            c.unicode()?;
            c.key()?;
        }
        b"Enmr" => {
            c.unicode()?;
            c.key()?;
            c.key()?;
            c.key()?;
        }
        b"rele" => {
            c.unicode()?;
            c.key()?;
            c.u32()?;
        }
        b"Idnt" | b"indx" => {
            c.u32()?;
        }
        b"name" => {
            c.unicode()?;
            c.key()?;
            c.unicode()?;
        }
        _ => return Err(DescriptorError::UnknownType(kind)),
    }
    Ok(())
}

#[cfg(test)]
/// Parses a bare descriptor (no block header) — what the tests and the
/// block parser share.
pub(super) fn parse_descriptor(
    data: &[u8],
    budget: &mut ItemBudget,
) -> Result<Descriptor, DescriptorError> {
    let mut c = Cursor { data, pos: 0 };
    descriptor_body(&mut c, 0, budget)
}

/// Parses an effects block (`lfx2`, `lfxs`, `lmfx`): `u32` object version
/// (any), `u32` descriptor version (`16`), then the descriptor.
pub(super) fn parse_effects_block(
    data: &[u8],
    budget: &mut ItemBudget,
) -> Result<Descriptor, DescriptorError> {
    let mut c = Cursor { data, pos: 0 };
    let _object_version = c.u32()?;
    if c.u32()? != 16 {
        return Err(DescriptorError::UnknownVersion);
    }
    descriptor_body(&mut c, 0, budget)
}

/// Parses a `vibA` or `blwh` block (0.241.0): a `u32` descriptor version
/// of `16` and the descriptor, with no object-version `u32` in front
/// (unlike [`parse_effects_block`]).
///
/// # Errors
///
/// A [`DescriptorError`] for truncated, over-budget or unknown data.
pub(super) fn parse_adjustment_block(
    data: &[u8],
    budget: &mut ItemBudget,
) -> Result<Descriptor, DescriptorError> {
    let mut c = Cursor { data, pos: 0 };
    if c.u32()? != 16 {
        return Err(DescriptorError::UnknownVersion);
    }
    descriptor_body(&mut c, 0, budget)
}

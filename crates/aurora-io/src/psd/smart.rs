//! Smart-object preservation, reader half (0.200.0, round S1).
//!
//! A smart-object layer's own tagged blocks (`SoLd`, `PlLd`, `SoLE`,
//! `fxrp`) are kept byte for byte, and so is the entry of the file's
//! document-level linked-layer list (`lnk2`, `lnkD`, `lnk3`, `lnkE`) that
//! holds the embedded file, matched through the unique ID the layer names.
//! Nothing in either is interpreted beyond finding that ID. See
//! `aurora_doc::smart_object` for the model and the policy for when a save
//! drops the data.
//!
//! **Bounded against hostile input.** A layer's blocks are limited per
//! block and per file; a linked list is walked entry by entry reading only
//! each entry's 8-byte length and the few bytes that hold its ID, and an
//! entry is read whole only when a layer names it and the pool budget
//! ([`aurora_doc::smart_object::MAX_LINKED_FILE_POOL_BYTES`]) and count
//! ([`aurora_doc::smart_object::MAX_LINKED_FILES`]) allow; a length that
//! runs past its list ends the walk. A smart object whose ID matches no
//! entry, or whose entry does not fit, is simply not preserved (the layer
//! opens as pixels and the open reports it, as before).
//!
//! **What is not preserved:** a layer that also carries smart filters
//! (`FXid`/`FEid`) — their document-level counterparts are not understood —
//! and one whose pixels are empty.

use std::collections::{BTreeMap, BTreeSet};

use aurora_doc::smart_object::{
    LINKED_LIST_KEYS, MAX_LINKED_FILE_POOL_BYTES, MAX_LINKED_FILES, MAX_SMART_OBJECT_BLOCK_BYTES,
    MAX_SMART_OBJECT_ID_BYTES, MAX_SMART_OBJECT_TOTAL_BLOCK_BYTES, SMART_OBJECT_BLOCK_KEYS,
};
use aurora_doc::{LinkedFile, SmartObjectBlock};

use super::descriptor::{self, ItemBudget, Value};
use super::{ByteSource, IoError, Record, Window};

/// A layer's smart object as the reader kept it: the unique ID and the
/// blocks, ready for `aurora_doc::SmartObject` once the pixels give it its
/// fingerprint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PsdSmartObject {
    pub unique_id: String,
    pub blocks: Vec<SmartObjectBlock>,
}

/// The kept blocks of one record, in [`SMART_OBJECT_BLOCK_KEYS`] order,
/// first of each kind; `None` when the record has no `SoLd`/`PlLd`/`SoLE`.
pub(super) fn kept_blocks(blocks: &[SmartObjectBlock]) -> Option<Vec<SmartObjectBlock>> {
    let mut out = Vec::new();
    for key in SMART_OBJECT_BLOCK_KEYS {
        if let Some(block) = blocks.iter().find(|b| b.key == key) {
            out.push(block.clone());
        }
    }
    let has_object = out.iter().any(|b| b.key != *b"fxrp");
    has_object.then_some(out)
}

/// The unique IDs a record's blocks name, in the order they are tried:
/// `SoLd`'s `Idnt`, `SoLE`'s, then `PlLd`'s.
pub(super) fn candidate_ids(blocks: &[SmartObjectBlock], budget: &mut ItemBudget) -> Vec<String> {
    let mut ids = Vec::new();
    for key in [*b"SoLd", *b"SoLE"] {
        if let Some(block) = blocks.iter().find(|b| b.key == key)
            && let Some(id) = descriptor_id(&block.data, budget)
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    if let Some(block) = blocks.iter().find(|b| b.key == *b"PlLd")
        && let Some(id) = placed_id(&block.data)
        && !ids.contains(&id)
    {
        ids.push(id);
    }
    ids
}

/// A usable ID: non-empty, within the bound, and made of printable ASCII
/// (Photoshop's are UUIDs).
fn clean_id(id: &str) -> Option<String> {
    let ok = !id.is_empty()
        && id.len() <= MAX_SMART_OBJECT_ID_BYTES
        && id.bytes().all(|b| (0x20..0x7f).contains(&b));
    ok.then(|| id.to_owned())
}

/// `SoLd`/`SoLE`: `soLD`, a `u32` version, the descriptor (version 16),
/// whose `Idnt` is the unique ID.
fn descriptor_id(data: &[u8], budget: &mut ItemBudget) -> Option<String> {
    let body = data.strip_prefix(b"soLD")?;
    let descriptor = descriptor::parse_effects_block(body, budget).ok()?;
    match descriptor.get(b"Idnt")? {
        Value::Text(text) => clean_id(text),
        _ => None,
    }
}

/// `PlLd`: `plcL`, a `u32` version, then the ID as a Pascal string.
fn placed_id(data: &[u8]) -> Option<String> {
    let rest = data.strip_prefix(b"plcL")?.get(4..)?;
    let (&len, text) = rest.split_first()?;
    let text = text.get(..usize::from(len))?;
    clean_id(std::str::from_utf8(text).ok()?)
}

/// Walks the located linked-layer lists and reads the entries named in
/// `wanted`, each as a framed [`LinkedFile`] (length, body, zero padding
/// to four bytes).
fn collect_linked(
    src: &mut dyn ByteSource,
    windows: &[([u8; 4], Window)],
    wanted: &BTreeSet<String>,
) -> Result<BTreeMap<String, LinkedFile>, IoError> {
    const LENGTH: usize = 8;
    // Kind (4), version (4), the ID's Pascal length byte and its bytes.
    const HEAD: usize = 4 + 4 + 1 + MAX_SMART_OBJECT_ID_BYTES;
    let mut found: BTreeMap<String, LinkedFile> = BTreeMap::new();
    let mut pool: u64 = 0;
    for (list, window) in windows {
        if !LINKED_LIST_KEYS.contains(list) {
            continue;
        }
        let mut w = *window;
        while w.remaining() >= (LENGTH + 9) as u64 {
            let start = w.pos;
            let length = u64::from_be_bytes(w.take_array::<8>(src, "linked layer length")?);
            let Ok(len) = usize::try_from(length) else {
                break;
            };
            if !w.fits(len) {
                break; // a damaged list: the rest cannot be located
            }
            let pad = (4 - len % 4) % 4;
            let head = w.peek(src, len.min(HEAD), "linked layer")?;
            let id = head
                .get(8)
                .and_then(|&n| head.get(9..9 + usize::from(n)))
                .and_then(|text| std::str::from_utf8(text).ok())
                .and_then(clean_id);
            // A last entry may lack its padding.
            let span = if w.fits(len + pad) { len + pad } else { len };
            if let Some(id) = id
                && wanted.contains(&id)
                && !found.contains_key(&id)
                && found.len() < MAX_LINKED_FILES
            {
                let framed = (LENGTH + len + pad) as u64;
                if pool.saturating_add(framed) <= MAX_LINKED_FILE_POOL_BYTES {
                    // One read, no second copy: the frame with its own
                    // padding when the list holds it (a missing final pad
                    // is the only case that grows the buffer).
                    let mut bytes = src.read_at(start, LENGTH + span, "linked layer")?;
                    bytes.resize(LENGTH + len + pad, 0);
                    pool += framed;
                    found.insert(
                        id.clone(),
                        LinkedFile {
                            list: *list,
                            unique_id: id,
                            entry: std::sync::Arc::new(bytes),
                        },
                    );
                }
            }
            w.advance(span, "linked layer")?;
        }
    }
    Ok(found)
}

/// Matches every record's smart-object blocks to an embedded file, sets
/// [`Record::smart`] for the ones that match and returns the files used, in
/// first-use order.
///
/// # Errors
///
/// [`IoError`] from reading a located entry.
pub(super) fn resolve(
    src: &mut dyn ByteSource,
    records: &mut [Record],
    windows: &[([u8; 4], Window)],
    budget: &mut ItemBudget,
) -> Result<Vec<LinkedFile>, IoError> {
    let mut candidates: Vec<Option<(Vec<String>, Vec<SmartObjectBlock>)>> = Vec::new();
    let mut wanted = BTreeSet::new();
    let mut block_bytes = 0usize;
    for record in records.iter() {
        let kept = if record.section.is_some() || record.smart_filters {
            None
        } else {
            kept_blocks(&record.smart_blocks)
        };
        let entry = kept.and_then(|blocks| {
            let size = blocks
                .iter()
                .map(|b| b.data.len())
                .fold(0usize, usize::saturating_add);
            let fits = blocks
                .iter()
                .all(|b| b.data.len() <= MAX_SMART_OBJECT_BLOCK_BYTES)
                && block_bytes.saturating_add(size) <= MAX_SMART_OBJECT_TOTAL_BLOCK_BYTES;
            if !fits {
                return None;
            }
            let ids = candidate_ids(&blocks, budget);
            if ids.is_empty() {
                return None;
            }
            block_bytes += size;
            wanted.extend(ids.iter().cloned());
            Some((ids, blocks))
        });
        candidates.push(entry);
    }
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let found = collect_linked(src, windows, &wanted)?;
    let mut used: Vec<String> = Vec::new();
    for (record, candidate) in records.iter_mut().zip(candidates) {
        let Some((ids, blocks)) = candidate else {
            continue;
        };
        if let Some(id) = ids.into_iter().find(|id| found.contains_key(id)) {
            if !used.contains(&id) {
                used.push(id.clone());
            }
            record.smart = Some(PsdSmartObject {
                unique_id: id,
                blocks,
            });
        }
    }
    Ok(used
        .into_iter()
        .filter_map(|id| found.get(&id).cloned())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(id: &str) -> Vec<u8> {
        let mut data = b"plcL".to_vec();
        data.extend_from_slice(&3u32.to_be_bytes());
        data.push(id.len() as u8);
        data.extend_from_slice(id.as_bytes());
        data.extend_from_slice(&[0, 0, 0, 1]);
        data
    }

    #[test]
    fn a_placed_layer_block_names_its_id() {
        let id = "20031581-1068-5645-bb2e-5dc035354e3b";
        assert_eq!(placed_id(&placed(id)).as_deref(), Some(id));
        assert_eq!(placed_id(b"plcL"), None);
        assert_eq!(placed_id(b"xxxx\0\0\0\x03\x02ab"), None);
        // The length byte runs past the data.
        assert_eq!(placed_id(b"plcL\0\0\0\x03\x20ab"), None);
        // Not printable ASCII.
        assert_eq!(placed_id(b"plcL\0\0\0\x03\x02a\n"), None);
    }

    #[test]
    fn kept_blocks_need_an_object_block_and_follow_the_fixed_order() {
        let b = |key: &[u8; 4]| SmartObjectBlock {
            key: *key,
            data: vec![1],
        };
        assert_eq!(kept_blocks(&[b(b"fxrp")]), None);
        assert_eq!(kept_blocks(&[]), None);
        let kept = kept_blocks(&[b(b"fxrp"), b(b"PlLd"), b(b"SoLd"), b(b"lfx2")]);
        let keys: Vec<[u8; 4]> = kept.unwrap_or_default().iter().map(|b| b.key).collect();
        assert_eq!(keys, vec![*b"SoLd", *b"PlLd", *b"fxrp"]);
    }
}

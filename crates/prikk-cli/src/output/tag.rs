//! `prikk tag list --format json` (RFC 146): `tag-list-v1`.

use crate::stdout::println;
use prikk_object::ObjectId;

use super::verification::escape_json_string;

/// One tag entry as the prose `tag list` loop already computes it: the tag ref's own name and the
/// block its tag object targets — exactly the two facts `print_tag_list`'s prose sibling prints,
/// nothing more.
pub(crate) struct TagListEntry {
    pub(crate) ref_name: String,
    pub(crate) target_block_id: ObjectId,
}

/// Print `prikk tag list --format json`. RFC 146 rule 4: `[]` on a repository with no tags is a
/// complete, valid answer at exit `0`, not an absent field.
pub(crate) fn print_tag_list_json(tags: &[TagListEntry]) {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"schema_version\": \"tag-list-v1\",\n");
    json.push_str("  \"tags\": [");
    for (index, tag) in tags.iter().enumerate() {
        if index > 0 {
            json.push(',');
        }
        json.push_str("\n    {\"ref_name\": ");
        json.push_str(&escape_json_string(&tag.ref_name));
        json.push_str(", \"target_block_id\": ");
        json.push_str(&escape_json_string(&tag.target_block_id.to_string()));
        json.push('}');
    }
    if !tags.is_empty() {
        json.push_str("\n  ");
    }
    json.push_str("]\n");
    json.push('}');
    println!("{json}");
}

//! Per-user tag scope: which owners' tags the engine is allowed to evaluate
//! (issue #267).
//!
//! The Hub keeps *visibility* unchanged — lists and pages always show every
//! tag. The scope only trims the set of tags that feed the **engine**
//! (similarity, ripeness/scoring, overlap tag filter, digging, recommend,
//! tag-queue).
//!
//! * [`TagScope::Own`] — only tags owned by the viewer (`owner_user_id = me`).
//! * [`TagScope::Collective`] — tags of the viewer plus everyone who shares at
//!   least one collective with them.
//!
//! A tag has **no** `collective_id`; the only direct mapping is
//! `hub_tags.owner_user_id`. Groups (`hub_tag_groups.collective_id`) assign
//! groups — and via `hub_group_tags` their tags — to a collective, but a tag
//! need not belong to any group, so the owner mapping is the authoritative
//! scope definition (see `plan-issue-267.md` §2).
//!
//! Resolution order ([`effective`]): `?tag_scope=` query param > live user
//! setting ([`crate::user_settings`]) > default `own`. The setting is read
//! fresh on every request, so toggling it works without a reload and without
//! any session/cache invalidation.

use sqlx::SqlitePool;

use crate::user_settings;

/// Which owners' tags the engine evaluates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagScope {
    Own,
    Collective,
}

impl TagScope {
    /// Parse a query value; unknown/empty -> `None` (fall back to the setting).
    pub fn parse(raw: Option<&str>) -> Option<TagScope> {
        match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("own") => Some(TagScope::Own),
            Some("collective") => Some(TagScope::Collective),
            _ => None,
        }
    }

    /// Interpret a stored setting value; missing/unknown -> [`TagScope::Own`].
    pub fn from_setting(raw: Option<&str>) -> TagScope {
        TagScope::parse(raw).unwrap_or(TagScope::Own)
    }

    /// Canonical query/setting value (`"own"` | `"collective"`).
    pub fn as_str(self) -> &'static str {
        match self {
            TagScope::Own => "own",
            TagScope::Collective => "collective",
        }
    }

    pub fn is_collective(self) -> bool {
        matches!(self, TagScope::Collective)
    }
}

/// Effective scope for a request: query param wins, else the live user setting,
/// else `own`. Reads the setting on every call — no caching.
pub async fn effective(pool: &SqlitePool, viewer_id: i64, query: Option<&str>) -> TagScope {
    if let Some(scope) = TagScope::parse(query) {
        return scope;
    }
    let raw = user_settings::get(pool, viewer_id, user_settings::TAG_SCOPE).await;
    TagScope::from_setting(raw.as_deref())
}

/// The `owner_user_id`s whose tags the engine may evaluate for `viewer_id`.
///
/// * `Own` -> `[viewer_id]`.
/// * `Collective` -> the viewer plus every user sharing any collective.
///
/// The returned list is sorted and de-duplicated (stable for callers/tests).
///
/// **Note:** an empty `owners` slice, when passed to the engine, means "no
/// owner filter" (legacy behaviour). `visible_owner_ids` never returns empty —
/// it always contains at least the viewer.
pub async fn visible_owner_ids(pool: &SqlitePool, viewer_id: i64, scope: TagScope) -> Vec<i64> {
    match scope {
        TagScope::Own => vec![viewer_id],
        TagScope::Collective => {
            let rows = sqlx::query_scalar::<_, i64>(
                "SELECT DISTINCT m2.user_id
                   FROM hub_collective_members m1
                   JOIN hub_collective_members m2 ON m2.collective_id = m1.collective_id
                  WHERE m1.user_id = ?1
                  ORDER BY m2.user_id",
            )
            .bind(viewer_id)
            .fetch_all(pool)
            .await
            .unwrap_or_default();
            let mut owners = rows;
            if !owners.contains(&viewer_id) {
                owners.push(viewer_id);
            }
            owners.sort_unstable();
            owners.dedup();
            owners
        }
    }
}

/// Convenience: resolve the effective scope *and* its owner list in one call.
pub async fn effective_owners(
    pool: &SqlitePool,
    viewer_id: i64,
    query: Option<&str>,
) -> (TagScope, Vec<i64>) {
    let scope = effective(pool, viewer_id, query).await;
    let owners = visible_owner_ids(pool, viewer_id, scope).await;
    (scope, owners)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_only_known_values() {
        assert_eq!(TagScope::parse(Some("own")), Some(TagScope::Own));
        assert_eq!(TagScope::parse(Some(" Collective ")), Some(TagScope::Collective));
        assert_eq!(TagScope::parse(Some("OWN")), Some(TagScope::Own));
        assert_eq!(TagScope::parse(Some("bogus")), None);
        assert_eq!(TagScope::parse(Some("")), None);
        assert_eq!(TagScope::parse(None), None);
    }

    #[test]
    fn from_setting_defaults_to_own() {
        assert_eq!(TagScope::from_setting(None), TagScope::Own);
        assert_eq!(TagScope::from_setting(Some("bogus")), TagScope::Own);
        assert_eq!(TagScope::from_setting(Some("collective")), TagScope::Collective);
    }

    #[test]
    fn as_str_roundtrips() {
        for s in [TagScope::Own, TagScope::Collective] {
            assert_eq!(TagScope::parse(Some(s.as_str())), Some(s));
        }
    }
}

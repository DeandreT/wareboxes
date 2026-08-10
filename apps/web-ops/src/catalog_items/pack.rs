use wareboxes_core::models::{Item, ItemPackLink};

pub(super) fn active_pack_links_for_item(
    links: &[ItemPackLink],
    item_id: i64,
) -> Vec<ItemPackLink> {
    let mut matching = links
        .iter()
        .filter(|link| {
            link.deleted.is_none()
                && (link.master_item_id == item_id || link.single_item_id == item_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    matching.sort_by_key(|link| link.id);
    matching
}

pub(super) fn pack_conversion_label(
    link: &ItemPackLink,
    items: &[Item],
    item_id: i64,
) -> (String, String) {
    let master = item_display_name(items, link.master_item_id);
    let single = item_display_name(items, link.single_item_id);
    if link.master_item_id == item_id {
        (
            format!("Contains {single}"),
            format!("1 {master} = {} x {single}", link.inner_qty),
        )
    } else {
        (
            format!("Used by {master}"),
            format!("1 {master} = {} x {single}", link.inner_qty),
        )
    }
}

fn item_display_name(items: &[Item], item_id: i64) -> String {
    items
        .iter()
        .find(|item| item.id == item_id)
        .and_then(|item| item.description.as_deref())
        .filter(|description| !description.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Item #{item_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: i64, description: &str) -> Item {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "tenant_id": 1,
            "created": "2026-01-01T00:00:00Z",
            "deleted": null,
            "description": description,
            "notes": null,
            "packaging_unit": "case",
            "dims_id": null,
            "pallet_hi": null,
            "pallet_ti": null,
            "inner_units": null,
            "skus": [],
            "barcodes": []
        }))
        .unwrap()
    }

    fn link(id: i64, master: i64, single: i64, deleted: bool) -> ItemPackLink {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "tenant_id": 1,
            "created": "2026-01-01T00:00:00Z",
            "deleted": deleted.then_some("2026-01-02T00:00:00Z"),
            "master_item_id": master,
            "single_item_id": single,
            "inner_qty": 6,
            "notes": null
        }))
        .unwrap()
    }

    #[test]
    fn relationships_include_both_directions_and_hide_deleted_rows() {
        let links = vec![
            link(3, 1, 2, false),
            link(1, 4, 1, false),
            link(2, 1, 5, true),
        ];
        let active = active_pack_links_for_item(&links, 1);
        assert_eq!(
            active.iter().map(|link| link.id).collect::<Vec<_>>(),
            vec![1, 3]
        );

        let items = vec![item(1, "Case"), item(2, "Each")];
        assert_eq!(
            pack_conversion_label(&links[0], &items, 1),
            ("Contains Each".to_owned(), "1 Case = 6 x Each".to_owned())
        );
    }
}

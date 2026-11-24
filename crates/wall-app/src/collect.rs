//! What the browser extension needs from the app: the boards to offer in its
//! menu, and noting where a collected picture or video came from.

use uuid::Uuid;

use crate::App;

/// A board the extension can send to.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardChoice {
    pub id: Uuid,
    pub name: String,
    /// 0 for top-level boards, 1 for boards inside them, and so on.
    pub depth: usize,
}

impl App {
    /// Every board of the library in the left panel's order.
    pub fn board_choices(&self) -> Vec<BoardChoice> {
        crate::sidebar::tree_order(&self.board_meta, &self.board_order)
            .into_iter()
            .map(|(id, depth)| BoardChoice { id, name: self.board_names.get(&id).cloned().unwrap_or_else(|| "Board".into()), depth })
            .collect()
    }

    /// Whether `id` is a board of this library.
    pub fn has_board(&self, id: Uuid) -> bool {
        self.board_meta.contains_key(&id)
    }

    /// Selects just this item.
    pub fn select_only(&mut self, id: Uuid) {
        self.selection = [id].into();
        self.needs_redraw = true;
    }

    /// Records the web page a picture or video item came from (shown as
    /// "Source" in the details panel) and its title. A picture collected
    /// before keeps its first source and title.
    pub fn set_item_source(&mut self, item: Uuid, url: Option<&str>, title: Option<&str>) {
        let Some(hash) = self.media_hash(&item) else { return };
        let Some(sc) = self.sidecars.get_mut(&hash) else { return };
        if sc.source_url.is_some() {
            return;
        }
        let title = title.map(str::trim).filter(|t| !t.is_empty());
        if url.is_none() && title.is_none() {
            return;
        }
        sc.source_url = url.map(str::to_owned);
        if let Some(t) = title {
            sc.title = Some(t.to_owned());
        }
        if let Some(lib) = &self.library {
            if let Err(e) = lib.write_sidecar(sc) {
                eprintln!("wall: could not save the source of {hash}: {e}");
            }
        }
        self.needs_redraw = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boards_are_offered_in_tree_order_and_sources_are_kept() {
        let mut app = App::demo(0);
        let root = app.board_id();
        let child = app.new_board();
        let list = app.board_choices();
        assert_eq!(list.iter().map(|b| (b.id, b.depth)).collect::<Vec<_>>(), vec![(root, 0), (child, 1)]);
        assert!(app.has_board(child));
        assert!(!app.has_board(Uuid::new_v4()));

        let mut png = Vec::new();
        image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 10, 10, 255])).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let id = app.import_image_bytes(&png, Some("x.png"), 1).unwrap();
        app.set_item_source(id, Some("https://example.com/a"), Some("A page"));
        app.set_item_source(id, Some("https://example.com/b"), None);
        let hash = app.media_hash(&id).unwrap();
        assert_eq!(app.sidecars[&hash].source_url.as_deref(), Some("https://example.com/a"));
        assert_eq!(app.sidecars[&hash].title.as_deref(), Some("A page"));
    }
}

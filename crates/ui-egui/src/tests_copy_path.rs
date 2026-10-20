//! Photo ▸ Copy Full Path (and the photo and folder context menus' item): the selected photos'
//! originals go on the clipboard, one full path per line; a folder row copies its folder.

use serde_json::{Value, json};

use crate::headless::Headless;
use crate::menubar::{MenuNode, run_item};
use crate::{LightcraftApp, Services};

/// A library of `/photos/trip/a.jpg`, `/photos/trip/b.jpg` and one demo scene.
fn app() -> Headless {
    use lightcraft_catalog::{Op, Photo, Source};
    let mut session = lightcraft_engine::Session::new();
    for source in [Source::File { path: "/photos/trip/a.jpg".into() }, Source::File { path: "/photos/trip/b.jpg".into() }, Source::Demo { scene: 0 }]
    {
        let id = session.catalog.alloc_photo_id();
        let p = Photo::new(id, source, "x.jpg", "JPEG", 8, 8, "2026-01-01T10:00:00");
        session.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    let mut h = Headless::new(LightcraftApp::new(session, Services { png: None, ..Default::default() }), [1200.0, 800.0], 1.0);
    h.step();
    h
}

fn ids(h: &Headless) -> Vec<u64> {
    h.app.session.catalog.photos().map(|p| p.id.0).collect()
}

fn select(h: &mut Headless, ids: &[u64]) {
    h.app.session.execute("library.select", &json!({"ids": ids})).unwrap();
}

#[test]
fn copies_one_full_path_per_selected_photo() {
    let mut h = app();
    let all = ids(&h);
    // listed in the Photo menu, next to Show in Finder
    let bar = crate::menubar::menu_bar(&h.app);
    let photo = &bar.iter().find(|(t, _)| t == "Photo").unwrap().1;
    let items: Vec<&str> = photo.iter().filter_map(|n| if let MenuNode::Item { id, .. } = n { Some(id.as_str()) } else { None }).collect();
    let at = items.iter().position(|i| *i == "app.showInFinder").unwrap();
    assert_eq!(items.get(at + 1), Some(&"app.copyPath"));
    select(&mut h, &all);
    assert!(crate::menus::ui_enabled(&h.app, "app.copyPath"));
    let r = run_item(&mut h.app, "app.copyPath", Value::Null).unwrap();
    // the demo scene has no file: left out
    assert_eq!(r["count"], 2);
    h.step();
    assert_eq!(h.app.copied.as_deref(), Some("/photos/trip/a.jpg\n/photos/trip/b.jpg"));
}

#[test]
fn a_demo_scene_alone_has_no_path() {
    let mut h = app();
    let demo = *ids(&h).last().unwrap();
    select(&mut h, &[demo]);
    assert!(!crate::menus::ui_enabled(&h.app, "app.copyPath"));
    assert!(run_item(&mut h.app, "app.copyPath", Value::Null).is_err());
    h.step();
    assert_eq!(h.app.copied, None);
}

#[test]
fn a_folder_row_copies_its_folder() {
    let mut h = app();
    let r = run_item(&mut h.app, "app.copyPath", json!({"paths": ["/photos/trip"]})).unwrap();
    assert_eq!(r["count"], 1);
    h.step();
    let want = if cfg!(windows) { "\\photos\\trip" } else { "/photos/trip" };
    assert_eq!(h.app.copied.as_deref(), Some(want));
}

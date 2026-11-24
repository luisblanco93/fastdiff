use super::tests::viewer;
use super::*;
use egui_kittest::{Harness, kittest::Queryable};

fn document_tab(path: &str) -> ReviewTab {
    let mut tab = super::tests::fixture();
    tab.request.target = OpenTarget::Document;
    tab.request.repository = path.into();
    tab.has_repository = false;
    tab.document_mode = true;
    tab
}

/// An app with `count` document tabs named `file0.rs`, `file1.rs`, ..., the last one selected.
fn app_with_tabs(count: usize) -> Fastdiff {
    let mut app = viewer();
    app.tabs = (0..count)
        .map(|index| document_tab(&format!("/d/file{index}.rs")))
        .collect();
    app.selected_tab = count - 1;
    app
}

fn harness(app: Fastdiff, width: f32) -> Harness<'static, Fastdiff> {
    let mut harness = Harness::builder()
        .with_size(egui::vec2(width, 300.0))
        .build_state(|context, app| app.show(context), app);
    harness.run();
    harness
}

#[test]
fn tab_strip_scroll_style_is_solid_thin_and_fits_under_the_tabs() {
    let style = tab_strip_scroll_style(&egui::style::ScrollStyle::floating());
    assert!(!style.floating, "a floating bar is painted over the tabs");
    assert_eq!(style.bar_width, TAB_SCROLL_BAR_WIDTH);
    assert!(
        style.allocated_width() <= TAB_SCROLL_BAR_WIDTH + TAB_SCROLL_BAR_GAP,
        "the bar band is {}",
        style.allocated_width()
    );
}

#[test]
fn tab_strip_scroll_style_keeps_the_base_colours() {
    let mut base = egui::style::ScrollStyle::floating();
    base.foreground_color = !base.foreground_color;
    base.active_handle_opacity = 0.123;
    let style = tab_strip_scroll_style(&base);
    assert_eq!(style.foreground_color, base.foreground_color);
    assert_eq!(style.active_handle_opacity, 0.123);
}

fn content_top(harness: &Harness<'static, Fastdiff>) -> f32 {
    harness.get_by_label("Search document").rect().top()
}

#[test]
fn a_few_tabs_add_no_scroll_bar_band() {
    let two = harness(app_with_tabs(2), 1200.0);
    let three = harness(app_with_tabs(3), 1200.0);
    assert_eq!(content_top(&two), content_top(&three));
}

#[test]
fn overflowing_tabs_keep_their_place_and_the_row_grows_by_the_scroll_bar_band() {
    let few = harness(app_with_tabs(2), 700.0);
    let few_label = few.get_by_label("file0.rs").rect();
    let many = harness(app_with_tabs(20), 700.0);
    let label = many.get_by_label("file0.rs").rect();
    // The band is added below the tabs, so they neither move nor shrink.
    assert!(
        (label.top() - few_label.top()).abs() <= 0.5
            && (label.height() - few_label.height()).abs() <= 0.5,
        "tab label moved from {few_label:?} to {label:?}"
    );
    // The content below the tab row starts lower by the band, so nothing overlaps the bar.
    let band = tab_strip_scroll_style(&egui::style::ScrollStyle::floating()).allocated_width();
    assert!(
        (content_top(&many) - content_top(&few) - band).abs() <= 0.5,
        "content moved by {} instead of {band}",
        content_top(&many) - content_top(&few)
    );
}

#[test]
fn closing_tabs_until_they_fit_removes_the_scroll_bar_band() {
    let mut harness = harness(app_with_tabs(20), 700.0);
    let overflowing_top = content_top(&harness);
    harness.state_mut().tabs.truncate(2);
    harness.state_mut().selected_tab = 1;
    // The newly selected tab shows a loading spinner, which keeps repainting.
    harness.run_steps(6);
    let fitting = self::harness(app_with_tabs(2), 700.0);
    assert!(overflowing_top > content_top(&fitting));
    assert_eq!(content_top(&harness), content_top(&fitting));
}

#[test]
fn the_selected_last_tab_is_scrolled_into_the_visible_strip() {
    let mut harness = harness(app_with_tabs(20), 700.0);
    harness.run();
    harness.run();
    let last = harness.get_by_label("file19.rs").rect();
    // The strip ends where the window-wide controls begin.
    let strip_end = harness.get_by_label("UI −").rect().left();
    assert!(
        last.right() <= strip_end && last.left() >= 0.0,
        "selected tab spans {}..{}, the strip ends at {strip_end}",
        last.left(),
        last.right()
    );
}

/// The scroll bar track and the selected tab's underline, as painted last frame.
fn painted_bar_and_underline(harness: &Harness<'static, Fastdiff>) -> (egui::Rect, f32) {
    use egui::epaint::Shape;
    fn walk(shape: &Shape, bars: &mut Vec<egui::Rect>, underlines: &mut Vec<f32>) {
        match shape {
            Shape::Vec(list) => list.iter().for_each(|s| walk(s, bars, underlines)),
            Shape::Rect(r) if (r.rect.height() - TAB_SCROLL_BAR_WIDTH).abs() < 0.01 => {
                bars.push(r.rect)
            }
            Shape::LineSegment { points, stroke }
                if stroke.width == 2.0 && points[0].y == points[1].y =>
            {
                underlines.push(points[0].y)
            }
            _ => {}
        }
    }
    let (mut bars, mut underlines) = (Vec::new(), Vec::new());
    for clipped in &harness.output().shapes {
        walk(&clipped.shape, &mut bars, &mut underlines);
    }
    let bar = bars
        .into_iter()
        .find(|rect| rect.width() > 50.0 && rect.top() < 120.0)
        .expect("scroll bar track painted");
    (bar, underlines[0])
}

#[test]
fn the_scroll_bar_is_painted_below_the_tabs_not_over_them() {
    let mut harness = harness(app_with_tabs(20), 700.0);
    harness.run_steps(8);
    let (bar, underline) = painted_bar_and_underline(&harness);
    // The underline is 2 px wide, centred on the bottom edge of the selected tab.
    assert!(
        bar.top() >= underline + 1.0 - 0.01,
        "bar starts at {} but the tab ends at {underline}",
        bar.top()
    );
}

#[test]
fn the_status_bar_ends_with_the_app_version_at_any_width() {
    for width in [420.0, 1200.0] {
        let harness = harness(app_with_tabs(1), width);
        let label = harness.get_by_label(&version_label()).rect();
        assert!(
            label.right() <= width && label.right() >= width - 24.0,
            "{width}: version label at {label:?}"
        );
    }
}

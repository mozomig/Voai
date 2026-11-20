//! Convert the captured Quartz field rectangle to a display-relative GPUI popup.
use gpui::{App, Bounds, Pixels, PlatformDisplay, Point, Size, point, px, size};
use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;
use objc2_core_foundation::{CGPoint, CGRect};
use objc2_core_graphics::{CGDisplayBounds, CGEvent, CGMainDisplayID};
use std::rc::Rc;

pub(super) fn compact_bounds(
    anchor: Option<CGRect>,
    cx: &App,
) -> Option<(Rc<dyn PlatformDisplay>, Bounds<Pixels>)> {
    let display_at = |position: CGPoint| {
        cx.displays().into_iter().find(|display| {
            let area = CGDisplayBounds(display.id().into());
            position.x >= area.origin.x
                && position.x < area.origin.x + area.size.width
                && position.y >= area.origin.y
                && position.y < area.origin.y + area.size.height
        })
    };
    let target = anchor
        .and_then(|r| {
            let center = CGPoint::new(
                r.origin.x + r.size.width / 2.,
                r.origin.y + r.size.height / 2.,
            );
            display_at(center).map(|display| (display, r))
        })
        .or_else(|| {
            let event = CGEvent::new(None)?;
            let pointer = CGEvent::location(Some(&event));
            Some((
                display_at(pointer)?,
                CGRect::new(pointer, Default::default()),
            ))
        });
    let (display, target) = target.or_else(|| {
        let display = cx.primary_display()?;
        let area = CGDisplayBounds(display.id().into());
        Some((
            display,
            CGRect::new(
                CGPoint::new(
                    area.origin.x + area.size.width / 2.,
                    area.origin.y + area.size.height * 0.8,
                ),
                Default::default(),
            ),
        ))
    })?;
    let screen = CGDisplayBounds(display.id().into());
    let field = Bounds::new(
        point(
            px((target.origin.x - screen.origin.x) as f32),
            px((target.origin.y - screen.origin.y) as f32),
        ),
        size(px(target.size.width as f32), px(target.size.height as f32)),
    );
    let work = visible_frame(screen).unwrap_or_else(|| display.bounds());
    let dimensions = size(px(78.), px(30.));
    Some((
        display,
        Bounds::new(place(field, work, dimensions), dimensions),
    ))
}

fn visible_frame(display: CGRect) -> Option<Bounds<Pixels>> {
    let main_height = CGDisplayBounds(CGMainDisplayID()).size.height;
    for screen in NSScreen::screens(MainThreadMarker::new()?).iter() {
        let frame = screen.frame();
        let top = main_height - frame.origin.y - frame.size.height;
        if (frame.origin.x - display.origin.x).abs() > 0.5 || (top - display.origin.y).abs() > 0.5 {
            continue;
        }
        let usable = screen.visibleFrame();
        return Some(Bounds::new(
            point(
                px((usable.origin.x - frame.origin.x) as f32),
                px(
                    (frame.origin.y + frame.size.height - usable.origin.y - usable.size.height)
                        as f32,
                ),
            ),
            size(px(usable.size.width as f32), px(usable.size.height as f32)),
        ));
    }
    None
}

fn place(field: Bounds<Pixels>, work: Bounds<Pixels>, bubble: Size<Pixels>) -> Point<Pixels> {
    let gap = px(6.);
    let min = work.origin + point(gap, gap);
    let max = work.bottom_right() - point(bubble.width + gap, bubble.height + gap);
    let above = field.top() - bubble.height - gap;
    let y = if above >= min.y {
        above
    } else {
        field.bottom() + gap
    };
    point(
        (field.center().x - bubble.width / 2.).clamp(min.x, max.x.max(min.x)),
        y.clamp(min.y, max.y.max(min.y)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn keeps_bubble_near_field_inside_work_area() {
        let work = Bounds::new(point(px(0.), px(25.)), size(px(500.), px(400.)));
        let bubble = size(px(78.), px(30.));
        let field = |x, y| Bounds::new(point(px(x), px(y)), size(px(40.), px(20.)));
        assert_eq!(
            place(field(180., 200.), work, bubble),
            point(px(161.), px(164.))
        );
        assert_eq!(
            place(field(480., 25.), work, bubble),
            point(px(416.), px(51.))
        );
        assert_eq!(
            place(field(-20., 390.), work, bubble),
            point(px(6.), px(354.))
        );
    }
}

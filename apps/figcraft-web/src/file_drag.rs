//! Pointer updates missing from eframe's browser file-drop backend. File contents still go
//! through that backend; this small state machine only supplies the real drag/drop position.

use egui::{Event, Pos2, RawInput};

/// Browser client coordinates are CSS pixels, not physical pixels. Match eframe's pointer
/// conversion: subtract the canvas origin and divide by egui's UI zoom, never by device DPR.
pub(crate) fn position(client: [f64; 2], origin: [f64; 2], zoom: f32) -> Option<Pos2> {
    if !zoom.is_finite() || zoom <= 0.0 {
        return None;
    }
    let pos = egui::pos2(((client[0] - origin[0]) / f64::from(zoom)) as f32, ((client[1] - origin[1]) / f64::from(zoom)) as f32);
    pos.is_finite().then_some(pos)
}

/// Browser drag lifecycle events used to track file gestures independently of file reads.
#[derive(Clone, Copy)]
pub(crate) enum Phase {
    Enter,
    Over,
    Leave,
    Drop,
    Cancel,
}

/// Pending pointer and hover corrections applied when eframe next consumes browser input.
#[derive(Default)]
pub(crate) struct FileDrag {
    active: bool,
    pointer: Option<Event>,
    dropped: Option<Pos2>,
    clear_hovered: bool,
    clear_after_drop: bool,
}

impl FileDrag {
    /// Ignore text/internal drags. Some browsers omit the transfer on leave/end, so an already
    /// active file gesture can still finish without it. A leave into a descendant is not an exit.
    pub(crate) fn record(&mut self, phase: Phase, files: bool, pos: Option<Pos2>, related_inside: bool) -> bool {
        match phase {
            Phase::Enter | Phase::Over | Phase::Drop if files => {
                self.active = !matches!(phase, Phase::Drop);
                if matches!(phase, Phase::Drop) {
                    self.dropped = pos;
                    self.clear_hovered = true;
                    self.pointer = pos.is_none().then_some(Event::PointerGone);
                } else {
                    self.pointer = Some(pos.map(Event::PointerMoved).unwrap_or(Event::PointerGone));
                }
                true
            }
            Phase::Leave if self.active && !related_inside => self.cancel(),
            Phase::Cancel | Phase::Drop if self.active => self.cancel(),
            _ => false,
        }
    }

    /// Ends an active gesture and queues pointer/hover cleanup for the next input frame.
    fn cancel(&mut self) -> bool {
        self.active = false;
        self.pointer = Some(Event::PointerGone);
        self.clear_hovered = true;
        true
    }

    /// The drop position must survive through the frame that consumes the backend's files.
    /// Clear the synthetic pointer next frame, unless a real pointer event already replaced it.
    /// Returns whether a cleanup frame is needed; never reads or enqueues file contents.
    pub(crate) fn apply(&mut self, raw: &mut RawInput) -> bool {
        if self.active && raw.events.iter().any(|e| matches!(e, Event::Key { key: egui::Key::Escape, pressed: true, .. })) {
            self.cancel();
        }
        if self.clear_hovered {
            raw.hovered_files.clear();
            self.clear_hovered = false;
        }
        if let Some(pos) = self.dropped.take() {
            raw.events.push(Event::PointerMoved(pos));
            self.clear_after_drop = true;
            return true;
        }
        if self.clear_after_drop {
            self.clear_after_drop = false;
            if self.pointer.is_none()
                && !raw.events.iter().any(|e| matches!(e, Event::PointerMoved(_) | Event::PointerButton { .. } | Event::PointerGone))
            {
                raw.events.push(Event::PointerGone);
            }
        }
        if let Some(pointer) = self.pointer.take() {
            raw.events.push(pointer);
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_use_the_canvas_origin_and_egui_zoom_not_dpr() {
        assert_eq!(position([300.0, 180.0], [100.0, 20.0], 2.0), Some(egui::pos2(100.0, 80.0)));
        assert_eq!(position([80.0, 50.0], [100.0, 20.0], 0.5), Some(egui::pos2(-40.0, 60.0)));
        for zoom in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert!(position([10.0, 20.0], [0.0, 0.0], zoom).is_none());
        }
        assert!(position([f64::NAN, 20.0], [0.0, 0.0], 1.0).is_none());
    }

    #[test]
    fn text_and_internal_drags_do_not_change_input() {
        let mut drag = FileDrag::default();
        for phase in [Phase::Enter, Phase::Over, Phase::Leave, Phase::Drop, Phase::Cancel] {
            assert!(!drag.record(phase, false, Some(egui::pos2(30.0, 40.0)), false));
        }
        let mut raw = RawInput::default();
        assert!(!drag.apply(&mut raw));
        assert!(raw.events.is_empty());
    }

    #[test]
    fn the_actual_drop_position_wins_and_is_cleared_after_one_frame() {
        let mut drag = FileDrag::default();
        drag.record(Phase::Enter, true, Some(egui::pos2(10.0, 20.0)), false);
        drag.record(Phase::Over, true, Some(egui::pos2(30.0, 40.0)), false);
        drag.record(Phase::Drop, true, Some(egui::pos2(50.0, 60.0)), false);
        drag.record(Phase::Leave, true, None, false);
        let mut raw = RawInput { events: vec![Event::PointerMoved(egui::pos2(1.0, 2.0))], ..Default::default() };
        assert!(drag.apply(&mut raw));
        assert!(matches!(raw.events.last(), Some(Event::PointerMoved(p)) if *p == egui::pos2(50.0, 60.0)));
        let mut next = RawInput::default();
        assert!(!drag.apply(&mut next));
        assert!(matches!(next.events.as_slice(), [Event::PointerGone]));
        let mut later = RawInput::default();
        drag.apply(&mut later);
        assert!(later.events.is_empty());
    }

    #[test]
    fn descendant_leave_is_ignored_but_an_exit_or_cancel_clears_hover() {
        for phase in [Phase::Leave, Phase::Cancel] {
            let mut drag = FileDrag::default();
            let pos = egui::pos2(10.0, 20.0);
            drag.record(Phase::Enter, true, Some(pos), false);
            assert!(!drag.record(Phase::Leave, true, None, true));
            let mut raw = RawInput::default();
            drag.apply(&mut raw);
            assert!(matches!(raw.events.as_slice(), [Event::PointerMoved(p)] if *p == pos));
            assert!(drag.record(phase, false, None, false), "missing transfer on exit still clears a file gesture");
            let mut raw = RawInput { hovered_files: vec![Default::default()], ..Default::default() };
            drag.apply(&mut raw);
            assert!(raw.hovered_files.is_empty());
            assert!(matches!(raw.events.as_slice(), [Event::PointerGone]));
        }
    }

    #[test]
    fn cleanup_does_not_hide_a_real_pointer_or_a_new_file_drag() {
        let mut drag = FileDrag::default();
        drag.record(Phase::Drop, true, Some(egui::pos2(10.0, 20.0)), false);
        drag.apply(&mut RawInput::default());
        let mut moved = RawInput { events: vec![Event::PointerMoved(egui::pos2(30.0, 40.0))], ..Default::default() };
        drag.apply(&mut moved);
        assert_eq!(moved.events.len(), 1);
        for _ in 0..3 {
            drag.record(Phase::Drop, true, Some(egui::pos2(10.0, 20.0)), false);
            drag.apply(&mut RawInput::default());
            drag.record(Phase::Enter, true, Some(egui::pos2(50.0, 60.0)), false);
            let mut entered = RawInput::default();
            drag.apply(&mut entered);
            assert!(matches!(entered.events.as_slice(), [Event::PointerMoved(p)] if *p == egui::pos2(50.0, 60.0)));
        }
    }
}

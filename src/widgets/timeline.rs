use crate::core::video::ResizeHandle;
use crate::core::video::Video;
use crate::core::video::ZOOM_ANIMATION_NSEC;
use adw::prelude::PreferencesRowExt;
use ges::prelude::*;
use gettextrs::gettext;
use gtk::prelude::BoxExt;
use gtk::prelude::ButtonExt;
use gtk::prelude::PopoverExt;
use gtk::prelude::WidgetExt;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene};
use std::{cell::RefCell, rc::Rc};

#[derive(Clone, Debug)]
pub struct DragState {
    effect_id: usize,
    handle: ResizeHandle,
    original_start_nsec: u64,
    original_end_nsec: u64,
}

mod imp {
    use std::cell::{OnceCell, Ref, RefMut};

    use gtk::gdk::Cursor;

    use crate::core::video::ZoomEffect;

    use super::*;

    #[derive(Default)]
    pub struct Timeline {
        video: OnceCell<Rc<RefCell<Video>>>,
        drag_state: Rc<RefCell<Option<DragState>>>,
        pango_layout: RefCell<Option<gtk::pango::Layout>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Timeline {
        const NAME: &'static str = "VideoTimeline";
        type Type = super::Timeline;
        type ParentType = gtk::Widget;

        fn new() -> Self {
            Self {
                video: OnceCell::new(),
                drag_state: Rc::new(RefCell::new(None)),
                pango_layout: RefCell::new(None),
            }
        }
    }

    impl ObjectImpl for Timeline {
        fn constructed(&self) {
            self.parent_constructed();

            self.obj().add_tick_callback(|timeline, _clock| {
                timeline.queue_draw();

                glib::ControlFlow::Continue
            });

            self.setup_effect_gesture();
            self.setup_playhead_gesture();
            self.setup_resize_gesture();
            self.setup_hover_cursor();
        }
    }
    impl WidgetImpl for Timeline {
        fn realize(&self) {
            self.parent_realize();

            let context = self.obj().pango_context();
            let layout = gtk::pango::Layout::new(&context);
            self.pango_layout.replace(Some(layout));
        }

        // TODO: show timecodes
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            if self.video.get().is_none() {
                return;
            }

            let current_pos = self
                .video()
                .pipeline()
                .query_position::<gst::ClockTime>()
                .map(|t| t.nseconds())
                .unwrap_or(0);
            let duration = self.duration_nsec();

            #[allow(deprecated)]
            let mut accent_color = self
                .obj()
                .style_context()
                .lookup_color("accent_bg_color")
                .unwrap();
            accent_color.set_alpha(0.5);

            #[allow(deprecated)]
            let text_color = self.obj().style_context().color();

            let widget = self.obj();
            let width = widget.width() as f32;
            let height = widget.height() as f32;
            let time_scale = width as f64 / duration as f64;

            let mut bg_color = widget.color();
            bg_color.set_alpha(0.1);

            snapshot.append_color(&bg_color, &graphene::Rect::new(0.0, 0.0, width, height));

            for effect in self.video().zoom_effects() {
                let (x, eff_width) = effect.timeline_bounds_at_scale(time_scale, duration);

                let rect = graphene::Rect::new(x as f32, 0.0, eff_width as f32, height);
                snapshot.append_color(&accent_color, &rect);

                if eff_width > 25.0
                    && let Some(pango_layout) = self.pango_layout.borrow().as_ref()
                {
                    let zoom_text = format!(
                        "{:.1}, {:.1} ({:.1}x)",
                        effect.pos_x, effect.pos_y, effect.factor
                    );
                    pango_layout.set_text(&zoom_text);
                    snapshot.save();
                    snapshot.translate(&graphene::Point::new(
                        x as f32 + 10.0 * 2.0,
                        height / 2.0 + 3.0,
                    ));
                    snapshot.append_layout(pango_layout, &text_color);
                    snapshot.restore();
                }
            }

            if current_pos > 0 && current_pos <= duration {
                let playhead_x = (current_pos as f64) * time_scale;
                let playhead_rect = graphene::Rect::new(playhead_x as f32, 0.0, 2.0, height);
                snapshot.append_color(&gdk::RGBA::RED, &playhead_rect);
            }
        }
    }

    impl Timeline {
        pub fn setup(&self, video: Rc<RefCell<Video>>) {
            self.video.set(video).expect("failed to set video");
        }

        fn video(&self) -> Ref<'_, Video> {
            self.video.get().expect("undefined video").borrow()
        }

        fn video_mut(&self) -> RefMut<'_, Video> {
            self.video.get().expect("undefined video").borrow_mut()
        }

        fn duration_nsec(&self) -> u64 {
            self.video().duration().unwrap().nseconds()
        }

        fn find_effect_at(&self, click_x: f64, _click_y: f64) -> Option<usize> {
            let timeline_width = self.obj().width() as f64;
            let duration = self.duration_nsec();
            self.video()
                .zoom_effects()
                .iter()
                .position(|eff| eff.timeline_contains(click_x, timeline_width, duration))
        }

        fn find_resize_handle_at(
            &self,
            target_x: f64,
        ) -> Option<(usize, ZoomEffect, ResizeHandle)> {
            let timeline_width = self.obj().width() as f64;
            let duration = self.duration_nsec();
            let effects = self.video().zoom_effects();

            for (id, effect) in effects.iter().enumerate() {
                let (x, width) = effect.timeline_bounds(timeline_width, duration);

                if target_x >= x && target_x <= x + 10.0 {
                    return Some((id, effect.clone(), ResizeHandle::Left));
                } else if target_x >= x + width - 10.0 && target_x <= x + width {
                    return Some((id, effect.clone(), ResizeHandle::Right));
                }
            }
            None
        }

        fn setup_playhead_gesture(&self) {
            let gesture = gtk::GestureClick::new();

            gesture.set_button(gdk::ffi::GDK_BUTTON_PRIMARY as u32);

            let this = self.obj().clone();
            gesture.connect_pressed(move |_gesture, _n_press, x, _y| {
                if this.imp().find_resize_handle_at(x).is_some() {
                    return;
                }

                let timeline_width = this.width() as f64;
                let duration = this.imp().duration_nsec();
                let pipeline = this.imp().video().pipeline();
                let ratio = (x / timeline_width).clamp(0.0, 1.0);
                let seek_pos = (duration as f64 * ratio) as u64;
                let _ = pipeline.seek_simple(
                    gst::SeekFlags::FLUSH,
                    gst::ClockTime::from_nseconds(seek_pos),
                );
            });

            self.obj().add_controller(gesture);
        }

        fn setup_resize_gesture(&self) {
            let gesture = gtk::GestureDrag::new();
            gesture.set_button(gdk::ffi::GDK_BUTTON_PRIMARY as u32);

            let this = self.obj().clone();
            gesture.connect_drag_begin(move |gesture, _start_x, _start_y| {
                let (x, _) = gesture.start_point().unwrap_or((0.0, 0.0));

                let Some((effect_id, effect, handle)) = this.imp().find_resize_handle_at(x) else {
                    return;
                };

                let state = DragState {
                    effect_id,
                    handle,
                    original_start_nsec: effect.start_nsec,
                    original_end_nsec: effect.end_nsec,
                };
                *this.imp().drag_state.borrow_mut() = Some(state);
            });

            let this = self.obj().clone();
            gesture.connect_drag_update(move |_gesture, offset_x, _offset_y| {
                let drag_state = this.imp().drag_state.borrow();
                let Some(state) = drag_state.as_ref() else {
                    return;
                };

                let timeline_width = this.width() as f64;
                let duration = this.imp().duration_nsec();

                if timeline_width <= 0.0 || duration == 0 {
                    return;
                }

                let time_scale = duration as f64 / timeline_width;
                let delta_x = offset_x;
                let delta_nsec = (delta_x * time_scale) as i64;

                let video_duration = this.imp().video().duration_nsec();
                let mut video = this.imp().video_mut();
                let Some(effect) = video.zoom_effect_mut(state.effect_id) else {
                    return;
                };
                match state.handle {
                    ResizeHandle::Left => {
                        let new_start = (state.original_start_nsec as i64 + delta_nsec) as u64;
                        if new_start < effect.end_nsec && new_start > ZOOM_ANIMATION_NSEC {
                            effect.start_nsec = new_start;
                        } else if new_start < effect.end_nsec {
                            effect.start_nsec = ZOOM_ANIMATION_NSEC;
                        } else {
                            effect.start_nsec = effect.end_nsec;
                        }
                    }
                    ResizeHandle::Right => {
                        if -delta_nsec > state.original_end_nsec as i64 {
                            effect.end_nsec = effect.start_nsec;
                            return;
                        }
                        let new_end = (state.original_end_nsec as i64 + delta_nsec) as u64;
                        if new_end > effect.start_nsec
                            && new_end < video_duration - ZOOM_ANIMATION_NSEC
                        {
                            effect.end_nsec = new_end;
                        } else if new_end > effect.start_nsec {
                            effect.end_nsec = video_duration - ZOOM_ANIMATION_NSEC;
                        } else {
                            effect.end_nsec = effect.start_nsec;
                        }
                    }
                    ResizeHandle::None => {}
                }
            });

            let this = self.obj().clone();
            gesture.connect_drag_end(move |_gesture, _offset_x, _offset_y| {
                {
                    let drag_state = this.imp().drag_state.borrow();
                    let Some(state) = drag_state.as_ref() else {
                        return;
                    };
                    if let Err(e) = this.imp().video_mut().update_zoom_range(
                        state.effect_id,
                        state.original_start_nsec,
                        state.original_end_nsec,
                    ) {
                        tracing::warn!("failed to apply zoom effect changes: {:?}", e);
                    }
                }

                *this.imp().drag_state.borrow_mut() = None;
            });

            self.obj().add_controller(gesture);
        }

        fn setup_effect_gesture(&self) {
            // rightclick only
            let gesture = gtk::GestureClick::new();
            gesture.set_button(gdk::ffi::GDK_BUTTON_SECONDARY as u32);

            let this = self.obj().clone();
            gesture.connect_pressed(move |_gesture, _n_press, x, y| {
                if let Some(effect_id) = this.imp().find_effect_at(x, y) {
                    let video = this.imp().video();
                    let effect = video.zoom_effect(effect_id).unwrap();
                    let this_edit = this.clone();
                    let this_remove = this.clone();
                    this.imp().show_effect_popover(
                        effect,
                        x,
                        y,
                        move |factor, pos_x, pos_y| {
                            let mut video = this_edit.imp().video_mut();
                            if let Err(e) =
                                video.update_zoom_geometry(effect_id, factor, pos_x, pos_y)
                            {
                                tracing::warn!("failed to apply zoom effect changes: {:?}", e);
                            }
                        },
                        move || {
                            let mut video = this_remove.imp().video_mut();
                            if let Err(e) = video.remove_zoom(effect_id) {
                                tracing::warn!("failed to remove zoom effect: {:?}", e);
                            }
                        },
                    );
                }
            });

            self.obj().add_controller(gesture);
        }

        pub fn show_effect_popover<E, D>(
            &self,
            effect: &ZoomEffect,
            x: f64,
            y: f64,
            on_edit: E,
            on_delete: D,
        ) where
            E: Fn(f64, f64, f64) + 'static,
            D: Fn() + 'static,
        {
            let popover = gtk::Popover::new();
            let box_container = gtk::Box::new(gtk::Orientation::Vertical, 6);
            box_container.set_margin_start(12);
            box_container.set_margin_end(12);
            box_container.set_margin_top(12);
            box_container.set_margin_bottom(12);

            let list_box = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .build();
            list_box.add_css_class("boxed-list");
            box_container.append(&list_box);

            let factor_row = adw::SpinRow::with_range(1.0, 2.0, 0.1);
            factor_row.set_title(&gettext("Factor"));
            factor_row.set_value(effect.factor);
            list_box.append(&factor_row);

            let pos_x_row = adw::SpinRow::with_range(0.0, 1.0, 0.05);
            pos_x_row.set_title(&gettext("Horizontal pos"));
            pos_x_row.set_value(effect.pos_x);
            list_box.append(&pos_x_row);

            let pos_y_row = adw::SpinRow::with_range(0.0, 1.0, 0.05);
            pos_y_row.set_title(&gettext("Vertical pos"));

            pos_y_row.set_value(effect.pos_y);
            list_box.append(&pos_y_row);

            // TODO: define animation duration (individualy)

            popover.connect_closed(glib::clone!(
                #[weak]
                factor_row,
                #[weak]
                pos_x_row,
                #[weak]
                pos_y_row,
                move |_| {
                    let factor = factor_row.value();
                    let pos_x = pos_x_row.value();
                    let pos_y = pos_y_row.value();
                    on_edit(factor, pos_x, pos_y);
                }
            ));

            let delete_content = adw::ButtonContent::builder()
                .icon_name("user-trash-symbolic")
                .label(&gettext("Remove"))
                .build();
            let delete = gtk::Button::builder()
                .child(&delete_content)
                .hexpand(true)
                .build();
            delete.connect_clicked(glib::clone!(
                #[weak]
                popover,
                move |_| {
                    on_delete();
                    popover.popdown();
                }
            ));
            box_container.append(&delete);

            popover.set_child(Some(&box_container));
            let obj = self.obj();
            popover.set_parent(&*obj);

            let rect = gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
            popover.set_pointing_to(Some(&rect));

            popover.popup();
        }

        fn setup_hover_cursor(&self) {
            let motion = gtk::EventControllerMotion::new();

            let this = self.obj().clone();
            motion.connect_motion(move |_controller, x, _y| {
                if this.imp().drag_state.borrow().is_some() {
                    return;
                }

                if this.imp().find_resize_handle_at(x).is_some() {
                    this.imp().set_cursor_name("ew-resize");
                } else {
                    this.set_cursor(None);
                }
            });

            let this = self.obj().clone();
            motion.connect_leave(move |_controller| {
                this.set_cursor(None);
            });

            self.obj().add_controller(motion);
        }

        fn set_cursor_name(&self, name: &str) {
            self.obj()
                .set_cursor(Cursor::from_name(name, None).as_ref());
        }
    }
}

glib::wrapper! {
    pub struct Timeline(ObjectSubclass<imp::Timeline>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Timeline {
    pub fn new() -> Self {
        glib::Object::new()
    }
}

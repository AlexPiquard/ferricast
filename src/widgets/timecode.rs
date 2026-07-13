use crate::core::video::Video;
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use std::{cell::RefCell, rc::Rc};

mod imp {
    use super::*;
    use std::cell::{OnceCell, Ref};

    #[derive(Default)]
    pub struct Timecode {
        video: OnceCell<Rc<RefCell<Video>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Timecode {
        const NAME: &'static str = "VideoTimecode";
        type Type = super::Timecode;
        type ParentType = gtk::Button;

        fn new() -> Self {
            Self {
                video: OnceCell::new(),
            }
        }
    }

    impl ObjectImpl for Timecode {
        fn constructed(&self) {
            self.parent_constructed();

            let label = gtk::Label::new(Some("00:00:00:00 / 00:00:00:00"));
            label.add_css_class("monospace");
            self.obj().set_child(Some(&label));

            let this = self.obj().downgrade();
            let label_clone = label.downgrade();
            self.obj().add_tick_callback(move |_, _| {
                let Some(this) = this.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let Some(label) = label_clone.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let imp = this.imp();
                if imp.video.get().is_some() {
                    let pos = imp.video().current_position_nsec().unwrap_or(0);
                    let dur = imp.video().duration_nsec();
                    let fmt = |ns: u64, fps: Option<gst::Fraction>| {
                        let s = ns / 1_000_000_000;
                        let h = s / 3600;
                        let m = (s % 3600) / 60;
                        let sec = s % 60;
                        if let Some(fps) = fps {
                            let numer = fps.numer() as u64;
                            let denom = fps.denom() as u64;
                            let total_frames = ns * numer / (denom * 1_000_000_000);
                            let tc_fps = (numer + denom - 1) / denom;
                            let f = total_frames % tc_fps;
                            format!("{:02}:{:02}:{:02}:{:02}", h, m, sec, f)
                        } else {
                            format!("{:02}:{:02}:{:02}", h, m, sec)
                        }
                    };
                    let fps = imp.video().framerate();
                    label.set_text(&format!("{} / {}", fmt(pos, fps), fmt(dur, fps)));
                }
                glib::ControlFlow::Continue
            });
        }
    }

    impl WidgetImpl for Timecode {}
    impl ButtonImpl for Timecode {}

    impl Timecode {
        pub fn setup(&self, video: Rc<RefCell<Video>>) {
            self.video.set(video).expect("failed to set video");
        }

        fn video(&self) -> Ref<'_, Video> {
            self.video.get().expect("undefined video").borrow()
        }
    }
}

glib::wrapper! {
    pub struct Timecode(ObjectSubclass<imp::Timecode>)
        @extends gtk::Button, gtk::Widget,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

impl Timecode {
    pub fn new() -> Self {
        glib::Object::new()
    }
}

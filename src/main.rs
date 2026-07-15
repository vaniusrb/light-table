//! light-table — GPU-resident photo develop (wgpu + rust-gpu + egui).

mod app;
mod crop;
mod develop;
mod gpu;
mod image_io;
mod ui;

fn main() {
    app::run();
}

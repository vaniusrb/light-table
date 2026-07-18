//! light-table — GPU-resident photo develop (wgpu + rust-gpu + egui).

mod app;
mod color;
mod crop;
mod demosaic;
mod develop;
mod gpu;
mod image_io;
mod orient;
mod ui;

fn main() {
    app::run();
}

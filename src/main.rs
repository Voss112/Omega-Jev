#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod domain;
mod infra;
mod tools;
mod ui;

fn main() -> eframe::Result<()> {
    ui::run()
}

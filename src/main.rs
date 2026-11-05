use atcgame::ui::App;
use macroquad::prelude::*;

fn window_conf() -> Conf {
    Conf {
        window_title: "Air Traffic Control".to_owned(),
        window_width: 1500,
        window_height: 950,
        window_resizable: true,
        high_dpi: false,
        sample_count: 4,
        ..Default::default()
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    let mut app = App::new().await;
    loop {
        app.frame();
        next_frame().await;
    }
}

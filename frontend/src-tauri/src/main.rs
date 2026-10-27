// в release на Windows консоль скрыта; в debug она показывает паники
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    nivora_launcher_lib::run()
}

// Оконная подсистема в release: без атрибута exe получает консольную
// подсистему, и при каждом запуске рядом с окном появляется чёрная консоль.
// В debug подсистему не меняем — консоль показывает паники при разработке
// (в release их пишет panic-хук в %LOCALAPPDATA%\nivora-launcher\panic.log).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    nivora_launcher_lib::run()
}

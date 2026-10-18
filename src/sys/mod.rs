//! Связь с системой: свои пути (`paths`), где сеанс (`env`), активен ли он
//! (logind), DBus, Wayland, X11 и запуск внешних команд с таймаутом.

pub mod dbus;
pub mod env;
pub mod lock;
pub mod paths;
pub mod proc;
pub mod session;
pub mod wayland;
pub mod x11;

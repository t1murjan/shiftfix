//! Связь с системой: где сеанс, активен ли он (logind), DBus и запуск
//! внешних команд с таймаутом.

pub mod dbus;
pub mod env;
pub mod proc;
pub mod session;

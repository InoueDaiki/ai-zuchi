//! OS やモデルに依存しない部分。外部とは [`ports`] の trait 越しに話す。
//! 依存の向きは、各アダプタのクレート → `zuchi-core` だけにする。

pub mod calc;
pub mod check;
pub mod context;
pub mod ports;
pub mod types;

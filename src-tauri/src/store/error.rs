//! 存储层错误类型。

use thiserror::Error;

/// 存储层错误。
#[derive(Debug, Error)]
pub enum StoreError {
    #[error("数据库文件操作失败: {0}")]
    Io(#[from] std::io::Error),

    #[error("数据库操作失败: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("迁移失败: {0}")]
    Migration(String),
}

/// 存储层结果别名。
pub type Result<T> = std::result::Result<T, StoreError>;

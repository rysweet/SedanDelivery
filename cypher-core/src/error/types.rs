use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum TypeError {
    #[error("cannot coerce {from} to {to}")]
    CannotCoerce { from: String, to: String },

    #[error("'{value}' is not a valid {type_name}")]
    InvalidLiteral { value: String, type_name: String },
}

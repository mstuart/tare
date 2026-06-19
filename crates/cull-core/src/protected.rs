use serde::{Deserialize, Serialize};
use crate::segment::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtectedKind { Path, LineNumber, ErrorCode, NumericLiteral, NullVsEmpty }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtectedSpan { pub span: Span, pub kind: ProtectedKind }

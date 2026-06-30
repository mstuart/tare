"""tare — query-aware LLM context compression.

The compiled Rust extension lives at ``tare._tare``. All public functions are
re-exported here so ``import tare; tare.compress(...)`` works directly.
"""

from tare._tare import (
    compact_csv,
    compact_html,
    compact_lossy,
    compress,
    crush,
    deref_images,
    expand,
    skeletonize,
    slim_schema,
    telegraphic,
)

__all__ = [
    "compress",
    "skeletonize",
    "compact_lossy",
    "slim_schema",
    "telegraphic",
    "compact_html",
    "compact_csv",
    "deref_images",
    "crush",
    "expand",
]

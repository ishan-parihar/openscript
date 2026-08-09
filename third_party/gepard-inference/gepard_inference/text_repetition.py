"""Adaptive text-repetition for short-utterance conditioning.

Inference-side copy of the training project's single source of truth for the
"repeat the text until it reaches a minimum text-token budget" technique. The
layout logic MUST agree byte-for-byte with training data prep, or the model
sees a train/inference mismatch and WER collapses.

The defect: on short inputs the K speaker-prefix tokens dominate the hidden
state and the 1–2 text tokens drown — the model never "latches" onto the
speech manifold → runaway / never-stop.

The fix: repeat the text block ``R-1`` extra times **before** the canonical
copy so the text region carries ~``target_text_tokens`` tokens of mass:

    [ (SOT text EOT) x (R-1) | SOT text EOT SOS | audio ... ]
      └──── context copies ──┘ └─── canonical ──┘
            (no SOS)            (only this one has SOS)

Only the canonical (last) copy carries SOS — the learned "audio starts now"
trigger — so context copies are read but never voiced. R is chosen from the
text-token count only, deterministically at inference.
"""

from __future__ import annotations

import math
from dataclasses import dataclass
from typing import List


@dataclass
class TextRepetitionConfig:
    """Config for adaptive text repetition. Disabled by default.

    Attributes:
        enabled: Master switch. False → ``target_R`` always returns 1 and
            ``build_input_ids`` reproduces the legacy single-copy layout.
        target_text_tokens: Repeat a short text until its text region holds
            ~this many text tokens.
        apply_below: Only texts with ``n_text_tokens < apply_below`` are
            repeated; longer texts keep R=1.
        max_repeats: Hard cap on R (bounds the prefill blow-up on 1-token inputs).
    """

    enabled: bool = False
    target_text_tokens: int = 16
    apply_below: int = 13
    max_repeats: int = 8

    def __post_init__(self) -> None:
        if self.target_text_tokens < 1:
            raise ValueError(f"target_text_tokens must be >= 1, got {self.target_text_tokens}")
        if self.apply_below < 1:
            raise ValueError(f"apply_below must be >= 1, got {self.apply_below}")
        if self.max_repeats < 1:
            raise ValueError(f"max_repeats must be >= 1, got {self.max_repeats}")

    @classmethod
    def from_config(cls, node) -> "TextRepetitionConfig":
        """Build from a plain dict / None. A missing node yields a disabled config."""
        if node is None:
            return cls()
        get = node.get
        return cls(
            enabled=bool(get("enabled", False)),
            target_text_tokens=int(get("target_text_tokens", 16)),
            apply_below=int(get("apply_below", 13)),
            max_repeats=int(get("max_repeats", 8)),
        )


class TextRepeater:
    """Builds the (possibly repeated) text-id layout. Stateless w.r.t. rows.

    Holds the config and the three special token ids that frame the text
    block (``start_of_text`` / ``end_of_text`` / ``start_of_speech``).
    """

    def __init__(
        self,
        config: TextRepetitionConfig,
        start_of_text: int,
        end_of_text: int,
        start_of_speech: int,
    ) -> None:
        self.config = config
        self.sot = int(start_of_text)
        self.eot = int(end_of_text)
        self.sos = int(start_of_speech)

    def target_R(self, n_text_tokens: int) -> int:
        """Deterministic repeat count from the text-token count.

        Returns 1 when disabled, when the text is already long enough
        (``>= apply_below``), or when it already meets the budget.
        """
        cfg = self.config
        if not cfg.enabled:
            return 1
        if n_text_tokens <= 0 or n_text_tokens >= cfg.apply_below:
            return 1
        if n_text_tokens >= cfg.target_text_tokens:
            return 1
        R = math.ceil(cfg.target_text_tokens / n_text_tokens)
        return max(1, min(R, cfg.max_repeats))

    def build_input_ids(self, text_token_ids: List[int], R: int) -> List[int]:
        """Assemble the text-id sequence for a given R.

            R == 1 → [ SOT, *text, EOT, SOS ]                       (legacy)
            R >  1 → [ SOT, *text, EOT ] * (R-1) + [ SOT, *text, EOT, SOS ]

        Only the final (canonical) copy carries SOS. The audio frames are
        appended by the caller; this returns exactly the text region.
        """
        if R < 1:
            raise ValueError(f"R must be >= 1, got {R}")
        text = list(text_token_ids)
        block = [self.sot] + text + [self.eot]
        canonical = block + [self.sos]
        if R == 1:
            return canonical
        return block * (R - 1) + canonical

    def expand(self, text_token_ids: List[int]) -> List[int]:
        """Convenience: pick the deterministic R then build the layout."""
        R = self.target_R(len(text_token_ids))
        return self.build_input_ids(text_token_ids, R)

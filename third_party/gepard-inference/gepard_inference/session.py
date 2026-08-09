"""Sessional TTS: load the model once, generate many times.

``GepardSession`` owns the runner (text → tokens) and the codec player (tokens
→ audio, and reference audio → tokens). Generation defaults come from
``config.yaml`` and are read once at startup; every call may override them.

Reference handling (see config.yaml):
    * the default voice is tokenized ONCE at ``load()`` and cached — never
      recomputed per request;
    * a call with ``reference=None`` reuses that cached default;
    * a call passing an audio path tokenizes that clip on first use and caches
      it by path, so repeat use of the same voice costs nothing.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, Optional, Tuple

import numpy as np
import torch
import yaml

from .codec_wrapper import Player
from .runner import GepardRunner


@dataclass
class SessionConfig:
    """Defaults read from ``config.yaml`` (never mutated by a request).

    Attributes:
        checkpoint: HF repo id (or local dir) of the self-describing checkpoint.
        attn_implementation: Backbone attention implementation for inference.
        defaults: Generation defaults ({temperature, cfg_scale, ...}).
        reference_audio: Default voice clip (relative paths resolve against the
            config file's directory). None → generate without a reference.
        max_ref_seconds: Reference clips are truncated to this before encoding.
        root: Directory of the config file; relative paths resolve against it.
    """

    checkpoint: str
    attn_implementation: str = "eager"
    defaults: Dict[str, Any] = field(default_factory=dict)
    reference_audio: Optional[str] = None
    max_ref_seconds: float = 60.0
    root: Path = field(default_factory=Path.cwd)

    @classmethod
    def from_yaml(cls, path: str | Path) -> "SessionConfig":
        path = Path(path)
        with open(path) as f:
            raw = yaml.safe_load(f) or {}
        model = raw.get("model") or {}
        if not model.get("checkpoint"):
            raise ValueError(f"{path}: model.checkpoint is required")
        # `reference_audio` lives alongside the generation defaults in the config
        # but is not a GepardRunner.generate kwarg — pull it out here.
        defaults = dict(raw.get("defaults") or {})
        reference_audio = defaults.pop("reference_audio", None)
        max_ref_seconds = float(defaults.pop("max_ref_seconds", 60.0))
        return cls(
            checkpoint=str(model["checkpoint"]),
            attn_implementation=str(model.get("attn_implementation", "eager")),
            defaults=defaults,
            reference_audio=reference_audio,
            max_ref_seconds=max_ref_seconds,
            root=path.parent.resolve(),
        )


class GepardSession:
    """Long-lived TTS session: model + codec in memory, cached references."""

    def __init__(self, config: SessionConfig, device: Optional[str] = None):
        self.config = config
        self.device = device
        self.runner: Optional[GepardRunner] = None
        self.player: Optional[Player] = None
        self._ref_cache: Dict[str, torch.Tensor] = {}
        self._default_ref: Optional[torch.Tensor] = None

    @classmethod
    def from_config(cls, path: str | Path = "config.yaml", device: Optional[str] = None) -> "GepardSession":
        """Load config + model in one shot."""
        return cls(SessionConfig.from_yaml(path), device=device).load()

    # ------------------------------------------------------------------
    # Startup (once)
    # ------------------------------------------------------------------

    def load(self) -> "GepardSession":
        """Load the runner + codec player and pre-encode the default voice."""
        dev = self.device or ("cuda" if torch.cuda.is_available() else "cpu")
        self.device = dev
        self.runner = GepardRunner.from_checkpoint(
            self.config.checkpoint,
            device=dev,
            attn_implementation=self.config.attn_implementation,
        )
        self.player = Player.from_checkpoint(self.config.checkpoint, device=dev)
        if self.config.reference_audio:
            self._default_ref = self._reference(self.config.reference_audio)
        print(
            f"[GepardSession] ready on {dev} | checkpoint={self.config.checkpoint} "
            f"| default voice={self.config.reference_audio}"
        )
        return self

    # ------------------------------------------------------------------
    # Per request
    # ------------------------------------------------------------------

    def synthesize(
        self,
        text: str,
        *,
        reference: Optional[str] = None,
        **overrides: Any,
    ) -> Tuple[int, np.ndarray]:
        """Synthesize ``text``; returns ``(sample_rate, waveform)`` (float32).

        Args:
            text: Text to speak.
            reference: Audio path to clone. None → the cached default voice.
            **overrides: Generation params overriding the config defaults
                (temperature, top_k, cfg_scale, cfg_frames, stop_threshold,
                max_frames, repetition_penalty, repetition_window). ``None``
                values are ignored (fall back to the default).
        """
        if self.runner is None or self.player is None:
            raise RuntimeError("GepardSession.load() must be called before synthesize()")
        if not (text or "").strip():
            raise ValueError("text is empty")

        ref_codes = self._reference(reference) if reference else self._default_ref

        params = dict(self.config.defaults)
        params.update({k: v for k, v in overrides.items() if v is not None})

        tokens = self.runner.generate(text, ref_codes=ref_codes, **self._gen_kwargs(params))
        return self.player.decode(tokens)

    # ------------------------------------------------------------------
    # Helpers
    # ------------------------------------------------------------------

    def _reference(self, audio_path: str) -> torch.Tensor:
        """Tokenize a reference clip, caching by resolved path."""
        p = Path(audio_path)
        if not p.is_absolute():
            p = self.config.root / p
        key = str(p)
        if key not in self._ref_cache:
            self._ref_cache[key] = self.player.encode_reference(
                key, max_seconds=self.config.max_ref_seconds
            )
        return self._ref_cache[key]

    @staticmethod
    def _gen_kwargs(params: Dict[str, Any]) -> Dict[str, Any]:
        """Coerce config/override values into GepardRunner.generate kwargs.

        ``cfg_frames == 0`` means "guide the whole utterance" → ``None``.
        """
        kw: Dict[str, Any] = {}
        for key, cast in (
            ("temperature", float), ("cfg_scale", float),
            ("stop_threshold", float), ("repetition_penalty", float),
            ("top_k", int), ("max_frames", int), ("repetition_window", int),
        ):
            if params.get(key) is not None:
                kw[key] = cast(params[key])
        if params.get("cfg_frames") is not None:
            cf = int(params["cfg_frames"])
            kw["cfg_frames"] = cf if cf > 0 else None
        return kw

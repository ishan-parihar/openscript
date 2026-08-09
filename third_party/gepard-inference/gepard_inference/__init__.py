"""Gepard inference package — a lean, self-contained extraction of the
inference-only classes from the `gepard-train` project.

Public surface:
    * :class:`~gepard_inference.session.GepardSession` — high-level sessional
      facade: model loaded once, config defaults, per-request overrides, cached
      references. What the CLI/server use.
    * :class:`~gepard_inference.runner.GepardRunner` — autoregressive runner
      with text classifier-free guidance; ``text → FSQ tokens``.
    * :class:`~gepard_inference.codec_wrapper.Player` — codec facade that reads
      its geometry from the checkpoint and handles the audio boundary the runner
      does not: ``reference audio → ref_codes`` and ``tokens → waveform``.

The runner (transformers) and the codec player (NeMo) are deliberately separate
models with separate lifecycles. Importing this package pulls both stacks; for a
token-only workflow import ``gepard_inference.runner`` directly to avoid NeMo.
"""

from .codec_wrapper import Player, UnfoldedCodecModel
from .runner import GepardRunner
from .session import GepardSession, SessionConfig

__all__ = ["GepardSession", "SessionConfig", "GepardRunner", "Player", "UnfoldedCodecModel"]

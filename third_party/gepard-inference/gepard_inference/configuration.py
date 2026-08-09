"""``GepardConfig`` — the self-describing model config (HF ``PretrainedConfig``).

Carries everything needed to reconstruct the model for inference without any
training YAML: the nested backbone config plus the TTS shape-drivers (audio
heads, audio_embed_dim, codec geometry, compressor dims, presence of the
voice-cloning stack). Stored as a separate ``gepard_config.json`` in the
checkpoint so it never collides with the backbone ``config.json``.
"""

from typing import Any, Dict, Optional

from transformers import PretrainedConfig

GEPARD_CONFIG_NAME = "gepard_config.json"


class GepardConfig(PretrainedConfig):
    model_type = "gepard"

    def __init__(
        self,
        backbone_config: Optional[Dict[str, Any]] = None,   # nested Qwen3.5 config dict
        audio_heads: Optional[Dict[str, int]] = None,       # ordered {channel: vocab}
        audio_embed_dim: int = 32,
        partial_rotary_factor: float = 1.0,                 # reconciled into the backbone on load
        special_tokens: Optional[Dict[str, int]] = None,    # bos_text/eot/bos_audio/tts_pad/...
        stop_loss_weight: float = 1.0,
        stop_pos_weight: float = 1.0,
        model_dtype: str = "bfloat16",
        codec: Optional[Dict[str, Any]] = None,             # {num_layers, fsq_levels, ...}
        text_repetition: Optional[Dict[str, Any]] = None,
        voice_cloning: Optional[Dict[str, Any]] = None,     # {enabled, compressor{...}, training{...}}
        **kwargs,
    ):
        self.backbone_config = dict(backbone_config or {})
        if isinstance(self.backbone_config.get("id2label"), dict):
            self.backbone_config["id2label"] = {
                str(k): v for k, v in self.backbone_config["id2label"].items()
            }
        self.audio_heads = {k: int(v) for k, v in (audio_heads or {}).items()}
        self.audio_embed_dim = int(audio_embed_dim)
        self.partial_rotary_factor = float(partial_rotary_factor)
        self.special_tokens = dict(special_tokens or {})
        self.stop_loss_weight = float(stop_loss_weight)
        self.stop_pos_weight = float(stop_pos_weight)
        self.model_dtype = str(model_dtype)
        self.codec = dict(codec or {})
        self.text_repetition = dict(text_repetition or {})
        self.voice_cloning = dict(voice_cloning or {})
        super().__init__(**kwargs)

    # ── convenience flags (mirror the model's conditional submodules) ──
    @property
    def vc_enabled(self) -> bool:
        return bool(self.voice_cloning.get("enabled", False))

    @property
    def supcon_head_present(self) -> bool:
        sc = (self.voice_cloning.get("training") or {}).get("supcon") or {}
        return self.vc_enabled and bool(sc.get("enabled")) and bool(sc.get("use_projection"))


def load_gepard_config(checkpoint_path: str) -> Optional["GepardConfig"]:
    """Read ``gepard_config.json`` from a local checkpoint dir or an HF Hub repo.

    Returns None when the checkpoint has no gepard config (legacy checkpoints
    are not supported by this inference-only package).
    """
    from .checkpoint_io import resolve_checkpoint_file

    path = resolve_checkpoint_file(checkpoint_path, GEPARD_CONFIG_NAME, required=False)
    if path is None:
        return None
    return GepardConfig.from_json_file(path)


def set_partial_rotary_factor(backbone_cfg, value: float) -> bool:
    """Force ``partial_rotary_factor`` into BOTH places the ecosystem reads it.

    Since transformers 5.x the model computes RoPE from
    ``config.rope_parameters["partial_rotary_factor"]`` — the flat top-level
    attribute is a legacy mirror the constructor does NOT keep in sync. So
    every write goes to both:

      - top-level ``partial_rotary_factor``            → what vLLM reads
      - ``rope_parameters["partial_rotary_factor"]``   → what the HF model reads

    Accepts a ``PretrainedConfig``/object or a plain dict. The nested key is
    only written when a ``rope_parameters`` dict already exists.
    Returns True if anything changed.
    """
    target = float(value)
    changed = False
    if isinstance(backbone_cfg, dict):
        if backbone_cfg.get("partial_rotary_factor") != target:
            backbone_cfg["partial_rotary_factor"] = target
            changed = True
        rope = backbone_cfg.get("rope_parameters")
        if isinstance(rope, dict) and rope.get("partial_rotary_factor") != target:
            rope["partial_rotary_factor"] = target
            changed = True
        return changed
    if getattr(backbone_cfg, "partial_rotary_factor", None) != target:
        backbone_cfg.partial_rotary_factor = target
        changed = True
    rope = getattr(backbone_cfg, "rope_parameters", None)
    if isinstance(rope, dict) and rope.get("partial_rotary_factor") != target:
        rope["partial_rotary_factor"] = target
        changed = True
    return changed


def reconcile_backbone_config(backbone_cfg, gepard_cfg: "GepardConfig") -> bool:
    """Patch the backbone config's ``partial_rotary_factor`` to the configured value.

    The full-attention build expects a specific rotary coverage, so on load the
    backbone config is forced to match ``gepard_cfg.partial_rotary_factor`` —
    flat AND nested (see ``set_partial_rotary_factor``). Returns True if changed.
    """
    return set_partial_rotary_factor(backbone_cfg, gepard_cfg.partial_rotary_factor)

"""Gepard multihead model — inference-only build.

Architecture:
  - Stock Qwen3.5 transformer backbone (no modifications)
  - N audio embedding tables (one per FSQ channel) → concat → GELU MLP →
    single frame embedding
  - N codebook output heads with per-channel vocabulary sizes
  - 1 binary stop head (end-of-speech prediction)
  - Optional voice-cloning stack: ``RefCompressor`` (codec ``ref_codes`` →
    K speaker prefix tokens) + learnable ``null_prefix``
  - No lm_head, no text generation, no training losses

This is a lean extraction of the training project's ``GepardModel``: the
module tree (and therefore the ``state_dict`` layout) matches published
checkpoints exactly, but every training-only code path (losses, diagnostics,
CFG dropout, gradient checkpointing plumbing) is removed. Generation is
driven externally by :class:`gepard_inference.runner.GepardRunner`.
"""

from __future__ import annotations

from types import SimpleNamespace
from typing import Dict, List, Optional

import torch
import torch.nn as nn

from transformers.models.qwen3_5.modeling_qwen3_5 import Qwen3_5TextModel
from transformers.models.qwen3_5.configuration_qwen3_5 import Qwen3_5TextConfig

from .configuration import GepardConfig, reconcile_backbone_config
from .ref_compressor import RefCompressor

# Aliases people actually write in configs → canonical torch dtypes.
_DTYPE_MAP = {
    "bfloat16": torch.bfloat16,
    "bf16": torch.bfloat16,
    "float16": torch.float16,
    "fp16": torch.float16,
    "half": torch.float16,
    "float32": torch.float32,
    "fp32": torch.float32,
    "float": torch.float32,
}


def resolve_dtype(name) -> torch.dtype:
    """Map a config dtype string (or a torch.dtype) to the torch dtype."""
    if isinstance(name, torch.dtype):
        return name
    try:
        return _DTYPE_MAP[str(name).lower().removeprefix("torch.")]
    except KeyError:
        raise ValueError(
            f"unknown model dtype {name!r}; expected one of {sorted(_DTYPE_MAP)}"
        ) from None


class SupConProjectionHead(nn.Module):
    """2-layer MLP projection head from compressor d_model to a SupCon embedding.

    Training-only module; declared here purely for state-dict compatibility
    with checkpoints trained with the supervised-contrastive objective (its
    weights are loaded but never used at inference).
    """

    def __init__(self, d_model: int, hidden_dim: int = 128, projection_dim: int = 128):
        super().__init__()
        self.fc1 = nn.Linear(d_model, hidden_dim, bias=True)
        self.fc2 = nn.Linear(hidden_dim, projection_dim, bias=False)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        return self.fc2(torch.relu(self.fc1(x)))


class GepardModel(nn.Module):
    """Multihead TTS model: Qwen3.5 backbone + N codebook heads + 1 stop head.

    Inference surface (used by the runner):
      - ``self.model`` — stock ``Qwen3_5TextModel`` backbone (KV-cached forward)
      - ``self._embed_audio`` — audio frame → backbone embedding
      - ``self.codebook_heads`` / ``self.vocab_sizes`` — per-channel logits
      - ``self.stop_head`` — end-of-speech logit
      - ``self.ref_compressor`` — voice-cloning prefix (may be None)
    """

    def __init__(
        self,
        config: Qwen3_5TextConfig,
        audio_heads: Dict[str, int],
        vc_config: Optional[SimpleNamespace] = None,
        codec: Optional[SimpleNamespace] = None,
        audio_embed_dim: int = 32,
    ):
        super().__init__()
        self.config = config
        self.audio_heads = audio_heads
        self.channel_names: List[str] = list(audio_heads.keys())
        self.vocab_sizes: List[int] = list(audio_heads.values())
        self.num_codebook_heads: int = len(audio_heads)
        hidden_size = config.hidden_size

        # Stock Qwen3.5 backbone
        self.model = Qwen3_5TextModel(config)

        # Audio input: per-codebook embeddings → concat → 2-layer GELU MLP.
        self.audio_embed_dim = audio_embed_dim
        self.audio_embeddings = nn.ModuleList([
            nn.Embedding(vocab_size, self.audio_embed_dim)
            for vocab_size in self.vocab_sizes
        ])
        self.audio_embed_proj = nn.Sequential(
            nn.Linear(self.num_codebook_heads * self.audio_embed_dim, hidden_size),
            nn.GELU(),
            nn.Linear(hidden_size, hidden_size),
            # Affine-free LayerNorm pins the frame-embedding scale.
            nn.LayerNorm(hidden_size, elementwise_affine=False),
        )
        # Rescales the unit-norm projection output to the text-embedding std.
        # Frozen parameter of shape [1]; the value comes from the checkpoint.
        self.audio_embed_scale = nn.Parameter(torch.tensor([1.0]), requires_grad=False)

        # Output heads: one Linear per channel, each with its own vocab size
        self.codebook_heads = nn.ModuleList([
            nn.Linear(hidden_size, vocab_size)
            for vocab_size in self.vocab_sizes
        ])
        self.stop_head = nn.Linear(hidden_size, 1)

        # Voice cloning: reference compressor + learnable null prefix (the
        # null prefix is a training-time CFG artifact kept for state-dict
        # compatibility; inference CFG contrasts text, not the speaker prefix).
        self.ref_compressor: Optional[RefCompressor] = None
        self.null_prefix: Optional[nn.Parameter] = None
        self.supcon_head: Optional[SupConProjectionHead] = None
        if vc_config is not None and vc_config.enabled:
            if codec is None:
                raise ValueError(
                    "GepardModel: vc_config.enabled=True but codec is None; "
                    "voice cloning needs the codec geometry."
                )
            self.ref_compressor = RefCompressor(
                codec=codec,
                compressor_cfg=vc_config.compressor,
                backbone_hidden_size=hidden_size,
            )
            self.null_prefix = nn.Parameter(
                torch.zeros(int(vc_config.compressor.num_queries), self.ref_compressor.d_model)
            )
            supcon = getattr(vc_config, "supcon", None)
            if supcon is not None and supcon.use_projection:
                self.supcon_head = SupConProjectionHead(
                    d_model=self.ref_compressor.d_model,
                    hidden_dim=int(supcon.projection_hidden_dim),
                    projection_dim=int(supcon.projection_dim),
                )

    def _embed_audio(self, audio_tensors: List[torch.LongTensor]) -> torch.FloatTensor:
        """Embed N codebook channels → concat → MLP → single frame embedding."""
        per_channel = [
            self.audio_embeddings[i](audio_tensors[i])
            for i in range(self.num_codebook_heads)
        ]
        concat = torch.cat(per_channel, dim=-1)            # [B, T_audio, N * audio_embed_dim]
        frame = self.audio_embed_proj(concat)              # [B, T_audio, d], unit-norm (LayerNorm)
        return frame * self.audio_embed_scale.to(frame.dtype)


def build_model(
    cfg: GepardConfig,
    attn_implementation: Optional[str] = None,
) -> GepardModel:
    """Reconstruct the model from a ``GepardConfig`` (fresh weights; matching shapes).

    Rebuilds the backbone from the nested config (reconciling
    ``partial_rotary_factor``). ``attn_implementation`` overrides the
    serialized backbone setting (inference typically wants ``eager`` where
    training used ``flash_attention_2``).
    """
    backbone_kwargs = dict(cfg.backbone_config)
    if attn_implementation is not None:
        # Drop any serialized attn setting so the override wins regardless of
        # which key form the transformers version wrote into to_dict().
        for k in ("attn_implementation", "_attn_implementation", "_attn_implementation_internal"):
            backbone_kwargs.pop(k, None)
        backbone_kwargs["attn_implementation"] = attn_implementation
    backbone = Qwen3_5TextConfig(**backbone_kwargs)
    reconcile_backbone_config(backbone, cfg)

    vc_config = None
    codec = None
    if cfg.vc_enabled:
        comp = cfg.voice_cloning.get("compressor", {})
        sc = (cfg.voice_cloning.get("training") or {}).get("supcon", {})
        vc_config = SimpleNamespace(
            enabled=True,
            compressor=SimpleNamespace(
                num_queries=int(comp["num_queries"]),
                num_layers=int(comp["num_layers"]),
                num_heads=int(comp["num_heads"]),
                d_model=comp.get("d_model"),
                ffn_hidden_size_multiplier=int(comp["ffn_hidden_size_multiplier"]),
                dropout=float(comp.get("dropout", 0.1)),
                queries_init_std=0.02,
            ),
            supcon=SimpleNamespace(
                use_projection=bool(sc.get("enabled", False)) and bool(sc.get("use_projection", False)),
                projection_hidden_dim=int(sc.get("projection_hidden_dim", 128)),
                projection_dim=int(sc.get("projection_dim", 128)),
            ),
        )
        # Duck-typed stand-in for the codec group: the compressor only reads
        # num_layers / fsq_levels / do_unfold.
        codec = SimpleNamespace(
            num_layers=int(cfg.codec["num_layers"]),
            fsq_levels=list(cfg.codec["fsq_levels"]),
            do_unfold=bool(cfg.codec.get("do_unfold", True)),
        )

    return GepardModel(
        config=backbone,
        audio_heads=dict(cfg.audio_heads),
        vc_config=vc_config,
        codec=codec,
        audio_embed_dim=cfg.audio_embed_dim,
    )

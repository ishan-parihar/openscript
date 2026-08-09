"""Shared codec operations — mixed-radix unfold and FSQ dequantization.

Mixed-radix decomposition (inverse of FSQ packing):
  A packed token ``k`` encodes ``len(fsq_levels)`` per-dimension codes as
      k = code_0 + code_1 * L_0 + code_2 * L_0*L_1 + ...
  so we recover
      code_d = (k // prod(L_0..L_{d-1})) % L_d
  (little-endian mixed base, consistent with the pipeline that produced the
  training dataset).
"""
from __future__ import annotations

from typing import Sequence

import numpy as np
import torch


def unfold_tokens(packed: torch.Tensor, num_levels: Sequence[int]) -> torch.Tensor:
    """Mixed-radix decomposition of packed codec token indices.

    Args:
        packed: (B, C, T) packed indices, long tensor.
        num_levels: FSQ levels per dimension within each codebook.

    Returns:
        (B, C * len(num_levels), T) per-dimension discrete codes.
        Channel order: for codebook c and FSQ dim d, output channel index is
        ``c * len(num_levels) + d``.
    """
    if packed.dim() != 3:
        raise ValueError(f"unfold_tokens expects [B, C, T], got {tuple(packed.shape)}")
    device = packed.device
    levels = torch.tensor(list(num_levels), device=device, dtype=torch.long)  # [D]
    bases = torch.tensor(
        np.cumprod([1] + list(num_levels[:-1])).tolist(),
        device=device,
        dtype=torch.long,
    )  # [D]

    B, C, T = packed.shape
    D = levels.shape[0]
    packed_ = packed.unsqueeze(2)                      # [B, C, 1, T]
    bases_ = bases.view(1, 1, D, 1)                    # [1, 1, D, 1]
    levels_ = levels.view(1, 1, D, 1)                  # [1, 1, D, 1]
    codes = (packed_ // bases_) % levels_              # [B, C, D, T]
    return codes.reshape(B, C * D, T)


def dequantize_codes(
    unfolded: torch.Tensor,
    num_levels: Sequence[int],
    num_layers: int,
) -> torch.Tensor:
    """Per-dimension symmetric dequantization of unfolded FSQ codes.

    Applies ``(x - L//2) / (L//2)`` per channel using the per-channel level
    pattern ``[num_levels * num_layers]``. Output lies in ``[-1, 1]`` by
    construction (codes are in ``[0, L-1]``).

    Args:
        unfolded: [..., C_total] int tensor (channel-last), where
            ``C_total = num_layers * len(num_levels)``.
        num_levels: FSQ levels per dimension within each codebook.
        num_layers: Number of codebook layers.

    Returns:
        Float tensor of the same shape as ``unfolded``, roughly in ``[-1, 1]``.
    """
    C_total = unfolded.shape[-1]
    expected = num_layers * len(num_levels)
    if C_total != expected:
        raise ValueError(
            f"dequantize_codes: last dim={C_total} but num_layers*len(num_levels)={expected}"
        )
    levels = torch.tensor(
        list(num_levels) * num_layers,
        device=unfolded.device,
        dtype=torch.float32,
    )  # [C_total]
    scale = (levels // 2).clamp_min(1.0)               # [C_total]
    x = unfolded.float()
    return (x - scale) / scale

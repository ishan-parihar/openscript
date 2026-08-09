"""NeMo audio-codec wrapper used at inference time.

``UnfoldedCodecModel`` extends NeMo's ``AudioCodecModel`` with direct decoding
from per-dimension discrete FSQ codes (the format the Gepard model produces),
bypassing mixed-radix composition/decomposition.

``Player`` is the high-level facade over it: given a Gepard checkpoint it reads
the codec geometry (``codec_id`` / ``fsq_levels`` / ``sample_rate``) straight
from the checkpoint's ``gepard_config.json`` — the user never names the codec —
and exposes the two audio-boundary operations the runner does not do itself:
reference-audio → ``ref_codes`` (encode) and generated tokens → waveform
(decode).
"""
from typing import Optional, Tuple

import numpy as np
import torch
from omegaconf import open_dict
from nemo.collections.tts.models import AudioCodecModel

from .codec_ops import unfold_tokens


class UnfoldedCodecModel(AudioCodecModel):
    """AudioCodecModel + decoding from unfolded per-dimension FSQ codes.

    Works with any GroupFiniteScalarQuantizer configuration — the number of
    groups, dimensions per group, and FSQ levels are read from the model's
    vector_quantizer at runtime.
    """

    def __init__(self, cfg, trainer=None):
        # SLMDiscriminator downloads microsoft/wavlm-base-plus (~360MB) and is
        # only used during training — strip it from the config before init.
        with open_dict(cfg):
            disc = cfg.get("discriminator", None)
            if disc is not None and "discriminators" in disc:
                disc.discriminators = [
                    d for d in disc.discriminators if "SLM" not in d._target_
                ]
        super().__init__(cfg, trainer)

    def decode_from_codes(self, codes: torch.Tensor, codes_len: torch.Tensor):
        """Decode audio from unfolded per-dimension discrete codes.

        Args:
            codes: (B, D, T) — per-dimension discrete values, where
                   D = num_groups * dims_per_group.
            codes_len: (B,) — valid frame count per batch element.

        Returns:
            audio: (B, T_audio) — decoded waveform
            audio_len: (B,) — valid audio lengths in samples
        """
        num_levels = self.vector_quantizer.fsqs[0].num_levels.squeeze()
        scale = (num_levels // 2).float().to(codes.device)

        groups = codes.chunk(self.vector_quantizer.num_groups, dim=1)
        dequantized = torch.cat(
            [(g - scale[None, :, None]) / scale[None, :, None] for g in groups],
            dim=1,
        )

        return self.decode_audio(inputs=dequantized, input_len=codes_len)


class Player:
    """Codec facade: reference-audio → codes, and generated tokens → waveform.

    ``GepardRunner`` produces only discrete FSQ tokens; the audio boundary lives
    here. A ``Player`` owns a NeMo codec configured entirely from the model
    checkpoint, so the two codec stages the runner does not do are:

        ref_codes = player.encode_reference("voice.wav")     # audio → codes
        tokens    = runner.generate(text, ref_codes=ref_codes)
        sr, wave  = player.decode(tokens)                    # codes → audio

    Kept separate from the runner on purpose: the codec is a different model
    (NeMo, heavy import) with its own lifecycle, so token generation and
    audio (de)coding stay independently loadable and testable.
    """

    def __init__(
        self,
        codec: "UnfoldedCodecModel",
        fsq_levels,
        sample_rate: int,
        device: torch.device,
    ):
        self.codec = codec
        self.fsq_levels = list(fsq_levels)
        self.sample_rate = int(sample_rate)
        self.device = torch.device(device)

    # ------------------------------------------------------------------
    # Factory
    # ------------------------------------------------------------------

    @classmethod
    def from_checkpoint(
        cls,
        checkpoint: str,
        device: Optional[str] = None,
    ) -> "Player":
        """Build a Player from a Gepard checkpoint (local dir or HF repo id).

        The codec repo id and geometry are read from the checkpoint's
        ``gepard_config.json`` ``codec`` block — the caller never names the
        codec. The NeMo codec is downloaded, moved to ``device`` and set to
        eval mode.

        Args:
            checkpoint: Same self-describing checkpoint the runner loads.
            device: 'cuda', 'cpu', or None (auto-detect).
        """
        from .configuration import load_gepard_config

        gepard_cfg = load_gepard_config(checkpoint)
        if gepard_cfg is None:
            raise FileNotFoundError(
                f"{checkpoint!r} has no gepard_config.json — the codec id is "
                "read from that file, so only self-describing checkpoints work."
            )
        codec = dict(gepard_cfg.codec or {})
        codec_id = codec.get("codec_id")
        if not codec_id:
            raise ValueError(
                f"{checkpoint!r}: gepard_config.json has no codec.codec_id; "
                "cannot locate the NeMo codec."
            )
        fsq_levels = list(codec.get("fsq_levels") or [])
        if not fsq_levels:
            raise ValueError(f"{checkpoint!r}: gepard_config.json has no codec.fsq_levels")
        sample_rate = int(codec.get("sample_rate", 22050))

        if device is None:
            device = "cuda" if torch.cuda.is_available() else "cpu"
        device = torch.device(device)

        model = UnfoldedCodecModel.from_pretrained(codec_id).eval().to(device)
        print(f"[Player] codec {codec_id} on {device} | fsq_levels={fsq_levels} sr={sample_rate}")
        return cls(codec=model, fsq_levels=fsq_levels, sample_rate=sample_rate, device=device)

    # ------------------------------------------------------------------
    # Audio boundary
    # ------------------------------------------------------------------

    def encode_reference(
        self,
        audio_path: str,
        max_seconds: Optional[float] = None,
    ) -> torch.Tensor:
        """Encode a reference clip into unfolded codec codes ``[1, T, C_total]``.

        Loaded mono at the codec sample rate, optionally truncated to
        ``max_seconds``, then codec-encoded and mixed-radix unfolded to the
        per-dimension channel layout the voice-cloning compressor consumes.
        Returned on CPU (the runner moves it to its own device).
        """
        import librosa

        wave_np, _ = librosa.load(audio_path, sr=self.sample_rate, mono=True)
        if max_seconds is not None:
            wave_np = wave_np[: int(max_seconds * self.sample_rate)]
        if wave_np.size == 0:
            raise ValueError(f"reference audio {audio_path!r} is empty")

        wave = torch.from_numpy(wave_np).unsqueeze(0).to(self.device)
        wave_len = torch.tensor([wave.shape[-1]], device=self.device)
        with torch.inference_mode():
            tokens, _ = self.codec.encode(audio=wave, audio_len=wave_len)  # [1, C, T]
        ref_codes = (
            unfold_tokens(tokens.cpu(), self.fsq_levels).permute(0, 2, 1).contiguous()
        )  # [1, T, C_total]
        return ref_codes

    def decode(self, tokens: torch.Tensor) -> Tuple[int, np.ndarray]:
        """Decode generated FSQ tokens ``(num_heads, T)`` into a waveform.

        Returns ``(sample_rate, waveform)`` with the waveform as a flat float32
        numpy array — ready for ``soundfile.write``.
        """
        codes = tokens.unsqueeze(0).to(self.device)          # (1, num_heads, T)
        codes_len = torch.tensor([codes.shape[-1]], device=self.device)
        with torch.inference_mode():
            audio, _ = self.codec.decode_from_codes(codes, codes_len)
        wave = audio.float().cpu().detach().flatten().numpy()
        return self.sample_rate, wave

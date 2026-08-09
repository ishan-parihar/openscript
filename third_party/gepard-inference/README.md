<div align="center">
  <img src="logo.png" alt="Gepard Logo" width="100%"/>
  <br><br>

  [![](https://dcbadge.limes.pink/api/server/https://discord.gg/NzP3rjB4SB?style=flat)](https://discord.gg/NzP3rjB4SB) [![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

# Gepard — Inference
Real-time, vLLM-native autoregressive text-to-speech. This repository is the reference **inference** stack: a small HTTP server that loads the model once and a lightweight client.
</div>

## About

**GEPARD** — **Ge**nerative, **P**rosody-aware, **A**utoregressive text-to-speech for **R**ealtime **D**ialogues — is a decoder-only TTS model built to be served by a *stock* LLM engine (vLLM) without custom CUDA kernels in the decode loop. A standard full-attention Qwen3.5 backbone predicts discrete audio codes; an FSQ-based NVIDIA NanoCodec turns them into a 22.05 kHz waveform. Everything non-standard — zero-shot voice cloning, text-repetition, classifier-free guidance — is kept out of the autoregressive loop or distilled into the weights, so generation stays **single-pass**.

Highlights:

- **vLLM-native, single-pass** frame generation — the whole audio frame (32 orthogonal FSQ channels) is sampled in one step, no depth-transformer.
- **Real-time on vLLM** — served through a stock vLLM engine, a single stream runs at ≈ 0.040 RTF with a ≈ 0.032 s time-to-first-audio (TTFA) — about 25× faster than real-time (RTX 5090) — and throughput scales to ≈ 204× aggregate on one server-class GPU under concurrent load.
- **CFG distilled via DPO** — the quality of two-pass classifier-free guidance is baked into the weights, so it costs nothing at serving time (CFG remains available as an optional quality lever).
- **Zero-shot voice cloning** — clone a voice from a short reference clip; the speaker profile is extracted once at prefill and never enters the decode loop. (In two-pass CFG mode voice similarity drops slightly — we're actively improving it.)

The design, architecture, and how inference works are documented in **[docs/MODEL_GUIDE.md](docs/MODEL_GUIDE.md)**. Full experimental detail is in the [technical report](tech_report_en.md).

## Installation

Requires a CUDA GPU and Python 3.12. The setup script builds a local `venv/` with a CUDA-matched PyTorch, the NeMo codec, and the Gepard inference package.

```bash
# 1. (once per machine, optional) system packages — nvcc, python3.12 headers, git-lfs
make system-deps

# 2. create venv/ and install the inference stack
make setup

# 3. authenticate Hugging Face
make login
```

## Quick Start

The server loads the model once and holds it in memory; the client fires requests against it. Use two terminals.

```bash
# terminal 1 — start the server (127.0.0.1:8000, interactive docs at /docs)
make serve

# terminal 2 — synthesize speech to a WAV
make generate ARGS="'Hi there! Great to finally meet you.' -o out.wav"
```

Clone a voice by pointing at a reference clip, and override generation knobs inline:

```bash
make generate ARGS="'Hello world' -o out.wav --reference ref_audio/audio_en.wav --cfg-scale 3"
```

You can also call the HTTP API directly (the server is a FastAPI app — see `/docs`):

```bash
curl -X POST http://127.0.0.1:8000/synthesize \
  -H "Content-Type: application/json" \
  -d '{"text": "Hello from Gepard."}' --output out.wav
```

Generation defaults (checkpoint, default voice, temperature, CFG, …) live in [`config.yaml`](config.yaml); every value is overridable per request. `inference_demo.ipynb` is an end-to-end notebook walkthrough.

## Commands

```bash
make help          # show all commands
make system-deps   # install apt deps (nvcc, python3.12, git-lfs) — sudo, once
make setup         # build venv/ and install the inference stack
make login         # authenticate Hugging Face
make serve         # start the TTS server (terminal 1)
make generate      # client: synthesize speech (terminal 2)
```

## Related Repositories

- **[gepard-train](https://github.com/nineninesix-ai/gepard-train)** — the training project: pretraining, fine-tuning, and DPO, with a detailed guide, code, and docs.
- **[gepard-vllm](https://github.com/nineninesix-ai/gepard-vllm)** — the vLLM serving "wheels": the production, continuous-batching inference path.

## Citation

If you use this work in your research, please cite:

```bibtex
@software{gepard_2026,
  author = {Abdurazakov Ulanbek, Pavlov Denis, and Bakashov Nursultan},
  title = {Gepard: Real-Time Decoder-Only TTS Native to vLLM},
  year = {2026},
  publisher = {Hugging Face},
  howpublished = {\url{https://huggingface.co/nineninesix/gepard-1.0}},
  note = {Open-source, vLLM-native autoregressive TTS}
}
```

## References

Gepard builds on and is inspired by a great deal of open work. The main pieces:

```bibtex
@misc{qwen3,
  title={Qwen3 Technical Report},
  author={Qwen Team},
  year={2025},
  eprint={2505.09388},
  archivePrefix={arXiv}
}

@inproceedings{kwon2023vllm,
  title={Efficient Memory Management for Large Language Model Serving with PagedAttention},
  author={Kwon, Woosuk and Li, Zhuohan and Zhuang, Siyuan and Sheng, Ying and Zheng, Lianmin and Yu, Cody Hao and Gonzalez, Joseph E and Zhang, Hao and Stoica, Ion},
  booktitle={Proceedings of the 29th Symposium on Operating Systems Principles (SOSP)},
  pages={611--626},
  year={2023},
  eprint={2309.06180},
  archivePrefix={arXiv}
}


@article{nvidia2025nanocodec,
  title={NanoCodec: Towards High-Quality Ultra Fast Speech LLM Inference},
  author={Casanova, Edresson and Neekhara, Paarth and Langman, Ryan and Hussain, Shehzeen and Ghosh, Subhankar and Yang, Xuesong and Juki{\'c}, Ante and Li, Jason and Ginsburg, Boris},
  journal={arXiv preprint arXiv:2508.05835},
  year={2025}
}

@article{mentzer2023fsq,
  title={Finite Scalar Quantization: VQ-VAE Made Simple},
  author={Mentzer, Fabian and Agustsson, Eirikur and Tschannen, Michael and Malireddy, Srikanth and Alshina, Elena},
  journal={arXiv preprint arXiv:2309.15505},
  year={2023}
}

@article{nvidia2024magpie,
  title={Improving Robustness of LLM-based Speech Synthesis by Learning Monotonic Alignment},
  author={Neekhara, Paarth and Hussain, Shehzeen and Ghosh, Subhankar and Li, Jason and Valle, Rafael and Badlani, Rohan and Ginsburg, Boris},
  journal={arXiv preprint arXiv:2406.17957},
  year={2024}
}

@article{ho2022cfg,
  title={Classifier-Free Diffusion Guidance},
  author={Ho, Jonathan and Salimans, Tim},
  journal={arXiv preprint arXiv:2207.12598},
  year={2022}
}

@article{rafailov2023dpo,
  title={Direct Preference Optimization: Your Language Model is Secretly a Reward Model},
  author={Rafailov, Rafael and Sharma, Archit and Mitchell, Eric and Ermon, Stefano and Manning, Christopher D and Finn, Chelsea},
  journal={arXiv preprint arXiv:2305.18290},
  year={2023}
}

@article{meng2024simpo,
  title={SimPO: Simple Preference Optimization with a Reference-Free Reward},
  author={Meng, Yu and Xia, Mengzhou and Chen, Danqi},
  journal={arXiv preprint arXiv:2405.14734},
  year={2024}
}



@article{voicestar2025,
  title={VoiceStar: Robust Zero-Shot Autoregressive TTS with Duration Control and Extrapolation},
  author={Peng, Puyuan and Li, Shang-Wen and Mohamed, Abdelrahman and Harwath, David},
  journal={arXiv preprint arXiv:2505.19462},
  year={2025}
}



```

## License

Apache 2.0 — see the [LICENSE](LICENSE) file for details.

Gepard loads the NVIDIA NeMo **NanoCodec** (`nvidia/nemo-nano-codec-22khz-1.89kbps-21.5fps`) at runtime. That model is not covered by Apache 2.0 — it is governed by the [NVIDIA Open Model License Agreement](https://developer.nvidia.com/licenses/nvidia-open-model-license-agreement-june-2024.pdf). See the [NOTICE](NOTICE) file for third-party attribution.

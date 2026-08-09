.PHONY: help system-deps setup login serve generate

# Use bash instead of sh for source command
SHELL := /bin/bash

# Gepard palette — cheetah coat (sand / rust / orange), grass green, red
RED    := \033[0;31m
GREEN  := \033[0;32m
YELLOW := \033[0;33m
ORANGE := \033[38;2;245;129;12m      # rusty orange (cheetah coat)
RUST   := \033[38;2;178;58;12m       # deep rust (spots)
SAND   := \033[38;2;224;178;92m      # tan fur
GRASS  := \033[38;2;104;159;56m      # grass green
BOLD   := \033[1m
RESET  := \033[0m
# Legacy aliases — remapped to the Gepard palette so existing help lines recolor
# without touching each one (cyan→sand, blue→rusty orange).
CYAN   := $(SAND)
BLUE   := $(ORANGE)

define BANNER
	@printf "\n"
	@printf "  $(ORANGE)$(BOLD) ██████╗ ███████╗██████╗  █████╗ ██████╗ ██████╗ $(RESET)\n"
	@printf "  $(ORANGE)$(BOLD)██╔════╝ ██╔════╝██╔══██╗██╔══██╗██╔══██╗██╔══██╗$(RESET)\n"
	@printf "  $(ORANGE)$(BOLD)██║  ███╗█████╗  ██████╔╝███████║██████╔╝██║  ██║$(RESET)\n"
	@printf "  $(ORANGE)$(BOLD)██║   ██║██╔══╝  ██╔═══╝ ██╔══██║██╔══██╗██║  ██║$(RESET)\n"
	@printf "  $(ORANGE)$(BOLD)╚██████╔╝███████╗██║     ██║  ██║██║  ██║██████╔╝$(RESET)\n"
	@printf "  $(ORANGE)$(BOLD) ╚═════╝ ╚══════╝╚═╝     ╚═╝  ╚═╝╚═╝  ╚═╝╚═════╝ $(RESET)\n"
	@printf "\n\n"
endef

# Default target
help:
	$(BANNER)
	@printf "\n"
	@printf "$(BOLD)$(GREEN)╔══════════════════════════════════════════════════╗$(RESET)\n"
	@printf "$(BOLD)$(GREEN)║                  NineNineSix AI                  ║$(RESET)\n"
	@printf "$(BOLD)$(GREEN)╚══════════════════════════════════════════════════╝$(RESET)\n"
	@printf "\n"
	@printf "$(BOLD)$(BLUE)Available commands:$(RESET)\n"
	@printf "  $(GREEN)make login$(RESET)         - Authenticate HuggingFace (needed for the model repo)\n"
	@printf "  $(GREEN)make system-deps$(RESET)   - Install apt deps (nvidia-cuda-toolkit + python3.12) — sudo, run once\n"
	@printf "  $(GREEN)make setup$(RESET)         - Setup inference env (venv: torch + transformers + NeMo codec)\n"
	@printf "  $(GREEN)make serve$(RESET)         - Start the TTS server (loads the model once) — terminal 1\n"
	@printf "  $(GREEN)make generate$(RESET)      - Client: synthesize speech from text — terminal 2\n"
	@printf "\n"
	@printf "$(BOLD)$(BLUE)Examples:$(RESET)\n"
	@printf "  $(CYAN)make serve$(RESET)                                     $(SAND)# terminal 1: hold the model$(RESET)\n"
	@printf "  $(CYAN)make generate ARGS=\"'Hi there!' -o out.wav\"$(RESET)     $(SAND)# terminal 2: fire requests$(RESET)\n"
	@printf "  $(CYAN)make generate ARGS=\"'Hello' -o out.wav --reference ref_audio/audio_en.wav --cfg-scale 3\"$(RESET)\n"
	@printf "\n"

# System-level apt deps (nvidia-cuda-toolkit + python3.12 headers). Opt-in, sudo,
# run once per machine (e.g. fresh AWS GPU box) BEFORE the venv setup.
system-deps:
	@printf "🔧 Installing system deps (apt, sudo)...\n"
	@sudo bash scripts/system_deps.sh

# Setup lean inference environment (venv: NeMo codec + runner, no train/whisper).
setup:
	@printf "🔧 Setting up inference environment (venv)...\n"
	@bash scripts/setup.sh
	@printf "✅ Inference env ready! Activate: source venv/bin/activate\n"

login:
	@printf "\n"
	@printf "$(BOLD)$(CYAN)╔══════════════════════════════════════════════════╗$(RESET)\n"
	@printf "$(BOLD)$(CYAN)║              Authentication Setup                ║$(RESET)\n"
	@printf "$(BOLD)$(CYAN)╚══════════════════════════════════════════════════╝$(RESET)\n"
	@printf "\n"
	@if [ ! -d "venv" ]; then \
		printf "$(RED)$(BOLD)✗ Error:$(RESET) $(RED)Virtual environment not found (venv/).$(RESET)\n"; \
		printf "$(YELLOW)  Run '$(BOLD)make setup$(RESET)$(YELLOW)' first.$(RESET)\n"; \
		printf "\n"; \
		exit 1; \
	fi
	@printf "$(BLUE)━━━ HuggingFace ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━$(RESET)\n"
	@printf "$(CYAN)  Configuring git credential helper...$(RESET)\n"
	@git config --global credential.helper store
	@printf "$(CYAN)  Launching HuggingFace login...$(RESET)\n"
	@printf "\n"
	@source venv/bin/activate && hf auth login
	@printf "\n"
	@printf "$(GREEN)  ✓ HuggingFace authentication done$(RESET)\n"
	@printf "\n"
	@printf "$(BOLD)$(GREEN)╔══════════════════════════════════════════════════╗$(RESET)\n"
	@printf "$(BOLD)$(GREEN)║            ✓ Authentication complete!            ║$(RESET)\n"
	@printf "$(BOLD)$(GREEN)╚══════════════════════════════════════════════════╝$(RESET)\n"
	@printf "\n"

# Start the TTS server (terminal 1). Loads the model once and holds it in memory.
#   make serve                       # 127.0.0.1:8000, docs at /docs
#   make serve ARGS="--port 8080 --reference ref_audio/audio_en.wav"
serve:
	@source venv/bin/activate && python serve.py $(ARGS)

# Client: send text to the running server, save a WAV (terminal 2).
#   make generate ARGS="'text to say' -o out.wav --reference ref_audio/audio_en.wav"
generate:
	@source venv/bin/activate && python cli.py $(ARGS)

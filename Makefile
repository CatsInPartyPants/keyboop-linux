BIN       := $(HOME)/.local/bin
UNIT_DIR  := $(HOME)/.config/systemd/user
AUTOSTART := $(HOME)/.config/autostart

.PHONY: build test install uninstall start stop logs

build:
	cargo build --release

test:
	cargo test

install: build
	install -Dm755 target/release/keyboop $(BIN)/keyboop
	install -Dm644 dist/keyboop.service $(UNIT_DIR)/keyboop.service
	install -Dm644 dist/keyboop-autostart.desktop $(AUTOSTART)/keyboop.desktop
	systemctl --user daemon-reload
	systemctl --user import-environment DISPLAY XAUTHORITY
	systemctl --user restart keyboop
	@echo "Установлено и запущено. Журнал: make logs"

uninstall:
	-systemctl --user stop keyboop
	rm -f $(BIN)/keyboop $(UNIT_DIR)/keyboop.service $(AUTOSTART)/keyboop.desktop
	systemctl --user daemon-reload
	@echo "Удалено. Настройки остались в ~/.config/keyboop"

start:
	systemctl --user import-environment DISPLAY XAUTHORITY
	systemctl --user restart keyboop

stop:
	systemctl --user stop keyboop

logs:
	journalctl --user -u keyboop -f

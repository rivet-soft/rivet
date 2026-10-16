RIVET — УСТАНОВКА НА macOS
==========================

1. Перетащите значок Rivet на папку «Программы» (Applications) в этом окне.

2. Откройте «Терминал» (Программы → Утилиты → Терминал), вставьте эту
   команду и нажмите Enter:

       xattr -dr com.apple.quarantine /Applications/Rivet.app

3. Запустите Rivet из «Программ» или через Spotlight (⌘ Пробел → Rivet).

Зачем нужна команда из шага 2
-----------------------------
Rivet пока не подписан сертификатом Apple, поэтому при первом запуске
macOS показывает «Apple не удалось подтвердить, что файл „Rivet“ не
содержит вредоносного ПО…». Команда снимает с приложения метку «скачано
из интернета», после чего оно открывается как обычно. Делать это нужно
один раз: обновления, которые Rivet ставит сам, этого не требуют.

Без Терминала: попробуйте открыть Rivet, закройте предупреждение, затем
«Системные настройки» → «Конфиденциальность и безопасность» → внизу
кнопка «Всё равно открыть» → подтвердите паролем.

Обновления
----------
Rivet сам проверяет новые версии на GitHub. Когда она выходит, на кнопке
над «Настройками» появляется точка: «Скачать» → «Перезапустить и
обновить». Все версии: https://github.com/rivet-soft/rivet/releases

Удаление
--------
Перетащите Rivet из «Программ» в Корзину. Данные (коллекции, окружения,
история) лежат в ~/Library/Application Support/Rivet — удалите эту
папку, если они больше не нужны.


RIVET — INSTALLING ON macOS
===========================

1. Drag the Rivet icon onto the Applications folder in this window.

2. Open Terminal (Applications → Utilities → Terminal), paste this
   command and press Enter:

       xattr -dr com.apple.quarantine /Applications/Rivet.app

3. Launch Rivet from Applications or Spotlight (⌘ Space → Rivet).

Step 2 is needed once because Rivet is not yet signed with an Apple
certificate, so macOS would otherwise say it "cannot verify that Rivet is
free of malware". Without Terminal: try to open Rivet, close the warning,
then System Settings → Privacy & Security → "Open Anyway".

Rivet updates itself from https://github.com/rivet-soft/rivet/releases

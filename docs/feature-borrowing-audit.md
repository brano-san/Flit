# Возможности для Flit из Double Commander, Explorer++ и Spacedrive

Аудит локальных исходников на 15 ноября 2026 года, очищен в тот же день. Цель — дополнительные пользовательские сценарии для Flit, которых в нём нет.

Из 147 пунктов первоначального аудита оставлено **68**. Убраны:
- уже реализованное или реализуемое прямо сейчас (агенты rest2 и todo);
- то, от чего отказались;
- нецелесообразное для Flit;
- то, что добавило бы постоянную нагрузку: Flit обязан оставаться быстрым.

**Правило производительности.** Ни один пункт не добавляет работы в кадр, в простой и в обычные операции:
- тяжёлые функции (поиск по содержимому, дубликаты, синхронизация и сравнение папок, карта занятого места, просмотр больших файлов, массовые операции) работают **только по явному запросу** пользователя, в фоне, с отменой;
- опции, которые замедляют обычные действия, выключены по умолчанию.

Список убранного — в конце документа. Идентификаторы оставшихся пунктов не менялись.

| Проект | Проверенная ревизия | Особенности |
|---|---|---|
| Flit | `7ee912315651fea1c1d566ed6bfde0d2526185d5`, v0.7.0, 2026-11-15 | Rust, собственный GPU UI, Windows/Linux, MIT. |
| Double Commander | `db3dc54ddbfd77a6fe648ae273e36467c151d438`, 2026-11-15 | Pascal/Lazarus; точные возможности зависят также от плагинов. |
| Explorer++ | `94ca53592de1c60fea76713ded62545bffec31c4`, 2026-11-12 | C++/Win32; часть функций выключена experimental feature flags. |
| Spacedrive | `4a1fa9ed50c49f413c02c6c9300df6acda727910`, 2026-11-14 | Переработанная версия v2. |

Ссылки ведут в локальные копии `D:\ToDelete`. Если эти каталоги будут удалены, для восстановления контекста понадобятся указанные ревизии. «Заимствовать» означает перенять пользовательский сценарий и спроектировать его в архитектуре Flit, а не переносить код.

## Что во Flit уже есть

Эти возможности исключены. В таблицах ниже предлагаются только конкретные расширения.

| Область | Уже реализовано | Проверенный код Flit |
|---|---|---|
| Навигация | Мультипанели, вкладки, дублирование и восстановление вкладок, поиск по заголовку и пути, сохранение раскладок и сеанса | [Реестр команд](D:/Projects/Flit/crates/flit-core/src/ui/actions.rs:203), [поиск вкладок](D:/Projects/Flit/crates/flit-core/src/ui/options.rs:311), [раскладки](D:/Projects/Flit/crates/flit/src/layouts.rs:26) |
| Виды | Детали, дерево, значки, сетка, карточки, колонки Миллера, группировка, сортировка, размеры папок и запоминание вида папки | [Виды](D:/Projects/Flit/crates/flit-core/src/ui/layouts.rs:46), [настройки вида](D:/Projects/Flit/crates/flit-core/src/ui/details.rs:272) |
| Поиск и фильтр | Индекс и обход, имя/путь/regex, расширение, размер, дата изменения, теги; Windows Search/AQS; фильтр имён с `*` и `?`; история запросов | [Запросы](D:/Projects/Flit/crates/flit-search/src/query.rs:27), [фильтр](D:/Projects/Flit/crates/flit-core/src/filter.rs:8), [история](D:/Projects/Flit/crates/flit-core/src/ui/search.rs:398) |
| Переименование | Пакетная буквальная замена, учёт регистра поиска, нумерация до/после имени с начальным значением, предпросмотр, проверка конфликтов, отмена | [batch_rename.rs](D:/Projects/Flit/crates/flit-core/src/ui/batch_rename.rs:28) |
| Операции | Копирование, перенос, корзина, отмена/повтор, пауза, отмена задания, retry/skip, конфликты, график скорости, ожидание занятого тома | [Задания](D:/Projects/Flit/crates/flit-core/src/ui/jobs.rs:63), [планирование по тому](D:/Projects/Flit/crates/flit-platform/src/local.rs:101), [Undo](D:/Projects/Flit/crates/flit-core/src/undo.rs:51) |
| Архивы | Просмотр, упаковка/распаковка, добавление в ZIP (в 7z и tar — у агента rest2, Е2), пароли, шифрование имён 7z, многотомность, параметры сжатия | [Форматы](D:/Projects/Flit/crates/flit-core/src/archive.rs:10), [диалог](D:/Projects/Flit/crates/flit-core/src/ui/archive.rs:255) |
| Контрольные суммы | CRC32, MD5, SHA-1/256/384/512; сравнение со строкой или другим файлом | [Алгоритмы](D:/Projects/Flit/crates/flit-core/src/digest.rs:4) |
| Метаданные | Цветные теги, tag-поиск, Git-колонки, колонки свойств оболочки Windows (сортировка и новые колонки — у rest2, W9), hidden/readonly/unblock, Unix owner/group/perms | [Теги](D:/Projects/Flit/crates/flit-core/src/tags.rs:8), [столбцы оболочки](D:/Projects/Flit/crates/flit-core/src/ui/mod.rs:237), [Unix-права](D:/Projects/Flit/crates/flit-core/src/ui/permissions.rs:135) |
| Ссылки | Ярлыки Windows, symlink Linux; symlink, junction и жёсткие ссылки на Windows — у rest2 (W10) | [Linux link](D:/Projects/Flit/crates/flit-platform-linux/src/ops.rs:957) |
| Предпросмотр | Текст/код/Markdown/RTF/изображения; shell handlers и медиа в Windows; медиа на Linux | [preview.rs](D:/Projects/Flit/crates/flit-core/src/preview.rs:14) |
| Сеть | UNC/SMB, WebDAV, gio/GVfs на Linux, сетевые диски; FTP на Windows — в todo (№15) | [URI](D:/Projects/Flit/crates/flit-core/src/ui/network.rs:8) |
| Интеграции | Свои команды с условиями, подменю и сочетаниями, меню оболочки, внешние редакторы, Git, автообновление, доступность | [user_menu.rs](D:/Projects/Flit/crates/flit-core/src/ui/user_menu.rs:70) |

Также есть: плоский просмотр вложенных папок, Flatten, выбор шрифта, Flit как файловый менеджер по умолчанию, восстановление закрытой вкладки с историей, печать через оболочку, миниатюры, терминал и терминал администратора, смена значка папки, автоперезагрузка превью, natural sort, папки первыми, переход по первым буквам, запуск команды из адресной строки, AND/NOT по тегам, процессы, удерживающие занятый файл.

## Как читать оценки

- **Новая** — встроенного сценария во Flit нет. **Расширение** — база есть, не хватает перечисленного.
- **S** — локальное расширение UI/модели; **M** — новая операция или существенная правка модуля; **L** — отдельная подсистема. Относительная сложность, не календарная оценка.
- **P1** — ближайшие кандидаты; **P2** — второй этап; **P3** — специализация.

## Переименование и выделение

Модель [Spec](D:/Projects/Flit/crates/flit-core/src/ui/batch_rename.rs:28) не содержит шаблонов, regex, пресетов и списков новых имён. Обычный фильтр скрывает строки; выбор по условию должен менять выделение, оставляя остальные строки видимыми.

| ID | Что добавить и зачем | Статус; оценка | Реализация у донора |
|---|---|---|---|
| R01 | Шаблоны имени и расширения отдельно: текст, basename, extension, части пути, имя родительской папки. Например, `Альбом_001.jpg`. | Расширение; P1/M | [DC: сборка имени](D:/ToDelete/doublecmd/src/fmultirename.pas:2087), [токены](D:/ToDelete/doublecmd/src/fmultirename.pas:2193) |
| R02 | Извлечение подстрок имени по диапазону, длине и индексам с конца с корректной обработкой Unicode. | Расширение; P2/M | [DC: sReplaceXX](D:/ToDelete/doublecmd/src/fmultirename.pas:1851) |
| R03 | Regex-переименование с группами подстановки, first/all и выбором, затрагивать ли расширение. | Расширение; P1/M | [DC: FreshText](D:/ToDelete/doublecmd/src/fmultirename.pas:2087) |
| R04 | Цепочка нескольких замен за один запуск. | Расширение; P2/S | [DC: несколько замен](D:/ToDelete/doublecmd/src/fmultirename.pas:2144) |
| R05 | Массовое изменение регистра имени и расширения: lower/upper, первая буква, первые буквы слов. | Расширение; P1/S | [DC: регистр](D:/ToDelete/doublecmd/src/fmultirename.pas:1984), [E++: rename tokens](D:/ToDelete/explorerplusplus/Explorer++/Explorer++/MassRenameHelper.cpp:16) |
| R06 | Шаг, ширина, заполнение нулями и место счётчика внутри шаблона. Начальное число уже есть. | Расширение; P1/S | [DC: токен C](D:/ToDelete/doublecmd/src/fmultirename.pas:2193) |
| R07 | Даты с пользовательским форматом и запрашиваемая при запуске переменная в шаблоне. | Расширение; P2/M | [DC: токены дат/V](D:/ToDelete/doublecmd/src/fmultirename.pas:2193) |
| R08 | Имена из метаданных: аудиотеги, EXIF, свойства документа; единый набор полей для переименования и колонок. | Расширение; P2/L | [DC: контентные токены](D:/ToDelete/doublecmd/src/fmultirename.pas:2193), [поля](D:/ToDelete/doublecmd/src/ufilefunctions.pas:183) |
| R09 | Именованные пресеты переименования, быстрый повтор и запуск из команды. | Расширение; P1/M | [DC: presets](D:/ToDelete/doublecmd/src/fmultirename.pas:974) |
| R10 | Новые имена из буфера/текстового файла, правка всего списка во внешнем редакторе с последующим предпросмотром. | Расширение; P1/M | [DC: import/edit names](D:/ToDelete/doublecmd/src/fmultirename.pas:2399) |
| R12 | Выделить/снять выделение по маскам, несколько масок OR, история масок. | Новая; P1/S–M | [E++: диалог](D:/ToDelete/explorerplusplus/Explorer++/Explorer++/WildcardSelectDialog.cpp:111), [применение](D:/ToDelete/explorerplusplus/Explorer++/Explorer++/ShellBrowser/ShellBrowserImpl.cpp:1436) |
| R13 | Выделить по тому же расширению, basename или пути; применить сохранённый поисковый шаблон к выделению. | Новая; P1/M | [DC: selection](D:/ToDelete/doublecmd/src/fileviews/ufileview.pas:1856), [E++: same type](D:/ToDelete/explorerplusplus/Explorer++/Explorer++/ListViewHandler.cpp:98) |
| R14 | Сохранить/восстановить выделение; импорт/экспорт списка выбранных файлов через буфер или файл. | Новая; P1/M | [DC: selection lists](D:/ToDelete/doublecmd/src/fileviews/ufileview.pas:1752) |

У Explorer++ «тот же тип» берёт **первый выбранный** объект. Для Flit нужно явно решить: эта семантика или объединение типов всех выделенных.

## Сравнение и синхронизация

Несколько панелей, сравнение хешей и политика ReplaceOlder не дают сравнения деревьев; такой подсистемы во Flit нет.

| ID | Что добавить и зачем | Статус; оценка | Реализация у донора |
|---|---|---|---|
| C01 | Быстро сравнить две панели по именам и времени изменения, выделить отсутствующие и более новые файлы. | Новая; P1/M | [DC: compare directories](D:/ToDelete/doublecmd/src/umaincommands.pas:4720). Одинаковая дата не доказывает равенство файлов. |
| C02 | Сравнить деревья и показать план до запуска: копировать влево/вправо, пропустить, удалить; ручная правка каждой строки. | Новая; P1/L | [DC: UI](D:/ToDelete/doublecmd/src/fsyncdirsdlg.pas:167), [модель](D:/ToDelete/doublecmd/src/usyncdirsmodel.pas:363) |
| C03 | Двустороннее сравнение и одностороннее зеркало, проверка содержимого, ignore-date, пустые папки, ограничение выделенным, ручное управление удалениями. | Новая; P1/L, развитие C02 | [DC: состояния](D:/ToDelete/doublecmd/src/usyncdirsmodel.pas:492), [исполнение](D:/ToDelete/doublecmd/src/usyncdirsservice.pas:1062) |
| C05 | Связанная навигация панелей: переход в подпапку одной стороны открывает такую же подпапку другой. | Новая; P1/M | [DC: SyncChangeDir](D:/ToDelete/doublecmd/src/umaincommands.pas:4568) |

## Поиск и рабочие коллекции

Собственный [Query](D:/Projects/Flit/crates/flit-search/src/query.rs:27) и [обход](D:/Projects/Flit/crates/flit/src/find.rs:65) не читают содержимое файлов. [Действия над результатами поиска](D:/Projects/Flit/crates/flit-core/src/ui/search.rs:850) ограничены открытием, переходом и копированием отдельного результата.

| ID | Что добавить и зачем | Статус; оценка | Реализация у донора |
|---|---|---|---|
| Q01 | Собственный поиск по содержимому: текст/regex, регистр, «не содержит», несколько кодировок; без системного индекса на обеих ОС. | Расширение; P1/L | [DC: content search](D:/ToDelete/doublecmd/src/ufindthread.pas:399) |
| Q03 | Поиск текста в OOXML/ODF без офисного пакета и системного индексатора. | Расширение; P2/L, вместе с Q01 | [DC: Office XML](D:/ToDelete/doublecmd/src/ufindthread.pas:405), [парсер](D:/ToDelete/doublecmd/src/uofficexml.pas:1) |
| Q04 | Поиск внутри архивов с результатом `архив → внутренний путь`. Только по отдельной настройке поиска, выключенной по умолчанию: иначе любой поиск начнёт открывать каждый архив. | Расширение; P2/L | [DC: архивы в поиске](D:/ToDelete/doublecmd/src/ufindthread.pas:658) |
| Q06 | Поиск дубликатов по имени, размеру и хешу/содержимому, группировка совпадений для уборки. Удаление групп — S13. | Новая; P1/L | [DC: duplicates](D:/ToDelete/doublecmd/src/ufindthread.pas:749), [ключи и hash](D:/ToDelete/doublecmd/src/ufindthread.pas:828) |
| Q07 | Расширенная область: глубина обхода, политика ссылок, атрибуты, несколько корней из открытых вкладок или выделенных папок. | Расширение; P1/M | [DC: параметры](D:/ToDelete/doublecmd/src/ffinddlg.pas:89), [обход](D:/ToDelete/doublecmd/src/ufindthread.pas:183) |
| Q08 | Именованные запросы/пресеты, с корнем или без него. | Расширение; P1/M | [DC: search templates](D:/ToDelete/doublecmd/src/ffinddlg.pas:937) |
| Q09 | Несколько независимых поисков с сохранёнными результатами и возвратом к прежней выдаче. | Расширение; P1/M | [DC: search instances](D:/ToDelete/doublecmd/src/umaincommands.pas:5447) |
| Q10 | Отправить найденные файлы в обычную панель и применять групповые операции: копирование, переименование, архивирование, удаление. | Расширение; P1/L | [DC: SearchResultFS](D:/ToDelete/doublecmd/src/ffinddlg.pas:2035), [MultiList](D:/ToDelete/doublecmd/src/filesources/multilist/umultilistfilesource.pas:152) |
| Q11 | Корзина сбора файлов — виртуальный набор ссылок из разных папок; добавлять постепенно, затем одна групповая операция. | Новая; P1/M–L | [DC: stash](D:/ToDelete/doublecmd/src/umaincommands.pas:5748), [источник](D:/ToDelete/doublecmd/src/filesources/stash/ustashfilesource.pas:99) |
| Q12 | Открыть внешний список путей как рабочую коллекцию: результаты сборки, сканера или скрипта. | Новая; P2/M, общая база с Q10/Q11 | [DC: load list](D:/ToDelete/doublecmd/src/umaincommands.pas:5631) |

## Копирование, очередь и целостность

Очередь по тому назначения, прогресс, пауза, отмена, Retry/Skip, проверка места и безопасная замена через временный файл уже есть. Пункты расширяют [Gate](D:/Projects/Flit/crates/flit-platform/src/local.rs:101), [OpCtl](D:/Projects/Flit/crates/flit-core/src/provider.rs:375) и движки операций.

| ID | Что добавить и зачем | Статус; оценка | Реализация у донора |
|---|---|---|---|
| O01 | Именованные очереди, перенос задания между ними, явный параллельный запуск. | Расширение; P1/M | [DC: operations manager](D:/ToDelete/doublecmd/src/uoperationsmanager.pas:339) |
| O02 | Порядок заданий, first/last, pause/cancel всей очереди. | Расширение; P1/M | [DC: queue UI](D:/ToDelete/doublecmd/src/fviewoperations.pas:710) |
| O05 | Возобновить прерванную копию с байтового смещения, со сверкой уже записанного префикса (у DC сверки нет — это требование Flit). | Расширение; P2/L | [DC: resume](D:/ToDelete/doublecmd/src/filesources/filesystem/ufilesystemutil.pas:643) |
| O06 | Политики конфликтов «заменить только меньший/больший» и сравнение прямо из конфликта. «Заменить более старый» уже есть. | Расширение; P2/M | [DC: conflict policies](D:/ToDelete/doublecmd/src/filesources/filesystem/ufilesystemutil.pas:1871) |
| O12 | Создавать и проверять манифесты контрольных сумм для набора/папки: файл на объект или общий манифест. | Расширение; P1/M | [DC: checksum dialog](D:/ToDelete/doublecmd/src/fchecksumcalc.pas:37), [проверка](D:/ToDelete/doublecmd/src/umaincommands.pas:2872) |

## Архивы

В [ArchiveOp](D:/Projects/Flit/crates/flit-core/src/archive.rs:221) только Compress/Extract/Open.

| ID | Что добавить и зачем | Статус; оценка | Реализация у донора |
|---|---|---|---|
| A01 | Проверить архив отдельной командой, без распаковки в целевую папку. | Расширение; P1/M | [DC: test archive](D:/ToDelete/doublecmd/src/umaincommands.pas:1410) |
| A02 | Отдельный архив на каждый выбранный файл/папку за один запуск. | Расширение; P1/S–M | [DC: separate archives](D:/ToDelete/doublecmd/src/fpackdlg.pas:508) |
| A03 | Переместить в архив: удалить исходники только после успешной упаковки. | Расширение; P2/M | [DC: PK_PACK_MOVE_FILES](D:/ToDelete/doublecmd/src/fpackdlg.pas:508) |
| A04 | Извлекать по маске и без сохранения путей. | Расширение; P2/M | [DC: extract options](D:/ToDelete/doublecmd/src/fextractdlg.pas:38) |

## Вкладки, закладки, таблица файлов и свойства

Закладки — [плоский Vec<Location>](D:/Projects/Flit/crates/flit-core/src/ui/sidebar.rs:199); [Tab](D:/Projects/Flit/crates/flit-core/src/ui/tab.rs:44) не содержит своего имени и блокировки. Даты в [Properties](D:/Projects/Flit/crates/flit-core/src/ui/properties.rs:210) только отображаются.

| ID | Что добавить и зачем | Статус; оценка | Реализация у донора |
|---|---|---|---|
| U01 | Запретить закрытие вкладки и заблокировать адрес с переходами в новую вкладку; режим возврата к исходной папке. | Новая; P1/M | [E++: LockState](D:/ToDelete/explorerplusplus/Explorer++/Explorer++/Tab.h:28), [DC: lock modes](D:/ToDelete/doublecmd/src/umaincommands.pas:2056) |
| U02 | Свои названия вкладок без переименования папки. | Новая; P1/S | [E++: customName](D:/ToDelete/explorerplusplus/Explorer++/Explorer++/Tab.h:44) |
| U03 | Закрыть дублирующие вкладки; политика переиспользования уже открытой папки. | Расширение; P2/S–M | [DC: duplicate tabs](D:/ToDelete/doublecmd/src/umaincommands.pas:1809) |
| U04 | Папки закладок, свои названия, менеджер, открыть все/сохранить все вкладки в папку; импорт/экспорт. | Расширение; P1/M–L | [E++: bookmark folders](D:/ToDelete/explorerplusplus/Explorer++/Explorer++/Bookmarks/UI/BookmarkContextMenu.cpp:54), [DC: import/export](D:/ToDelete/doublecmd/src/fhotdirexportimport.pas:1) |
| U06 | Копировать выделенные строки вида с колонками и заголовками в TSV; отдельно — выгрузить содержимое папки в текст. | Новая; P1/S–M | [E++: TSV](D:/ToDelete/explorerplusplus/Explorer++/Explorer++/MsgHandler.cpp:561), [listing](D:/ToDelete/explorerplusplus/Explorer++/Helper/FileOperations.cpp:264) |
| U07 | Копировать «универсальный» путь: путь на подключённом сетевом диске превращается в UNC. | Расширение; P1/S, Windows | [E++: WNetGetUniversalName](D:/ToDelete/explorerplusplus/Explorer++/Helper/ClipboardHelper.cpp:29) |
| U08 | Массовая смена Created/Modified/Accessed, рекурсивно; взять даты другого файла. | Расширение; P1/M | [DC: timestamps](D:/ToDelete/doublecmd/src/fsetfileproperties.pas:61), [E++: date UI](D:/ToDelete/explorerplusplus/Explorer++/Explorer++/SetFileAttributesDialog.cpp:93) |
| U09 | Атрибуты System/Archive/Not-indexed и состояния «оставить/включить/выключить» для смешанного выделения. | Расширение; P2/M | [E++: attributes](D:/ToDelete/explorerplusplus/Explorer++/Explorer++/SetFileAttributesDialog.cpp:221), [DC: tri-state](D:/ToDelete/doublecmd/src/fsetfileproperties.pas:321) |
| U11 | Колонки, которых нет и в W9: размер на диске, цель ссылки, имя 8.3, год и битрейт, тема, продукт; на Linux также владелец и число жёстких ссылок (они уже есть в `stat`). На Windows владельца и число ссылок не делать: это открытие дескриптора на каждую строку. Значения читаются только для видимых строк и только пока колонка показана. | Расширение; P2/M | [E++: ColumnType](D:/ToDelete/explorerplusplus/Explorer++/Explorer++/ShellBrowser/Columns.h:10) |

## Просмотр и команды

Текст в превью ограничен [64 КиБ/10 000 символов](D:/Projects/Flit/crates/flit-core/src/preview.rs:14). [Places](D:/Projects/Flit/crates/flit-core/src/cmdline.rs:3) не содержит второй панели и запроса параметров.

| ID | Что добавить и зачем | Статус; оценка | Реализация у донора |
|---|---|---|---|
| X01 | Просмотр hex/binary/decimal с адресами и поиском; переход к строке в режиме кода. Полнофайловый доступ — N28. | Новая; P2/L | [DC: viewer modes](D:/ToDelete/doublecmd/src/fviewer.pas:4270) |
| X06 | Настраиваемая панель инструментов: свои кнопки с подменю, бросок файлов на кнопку запускает приложение; перестановка и удаление штатных кнопок, сброс. | Расширение; P2/M | [E++: toolbar](D:/ToDelete/explorerplusplus/Explorer++/Explorer++/ApplicationToolbar.cpp:154), [customize](D:/ToDelete/explorerplusplus/Explorer++/Explorer++/CustomizableToolbarView.cpp:39). Сейчас [панель фиксирована](D:/Projects/Flit/crates/flit-core/src/ui/mod.rs:2614). |
| X07 | Макросы команд: путь второй панели, относительные списки, запрос параметра у пользователя. Не повторять `{paths}`, `{name}`, `{dir}` и `%F`. | Расширение; P2/M | [DC: macros](D:/ToDelete/doublecmd/src/ushellexecute.pas:563), [параметры](D:/ToDelete/doublecmd/src/ushellexecute.pas:817) |
| X09 | Именованные сетевые подключения: сервер, протокол, proxy, ключи и отпечаток хоста, переподключение. Пароли — в системном хранилище, не base64 как у DC. | Расширение; P2/L | [DC: connection manager](D:/ToDelete/doublecmd/src/fconnectionmanager.pas:1) |

## Из Spacedrive

| ID | Что добавить и зачем | Статус; оценка | Реализация и ограничения Spacedrive |
|---|---|---|---|
| S05 | Иерархия тегов, например `Работа/Клиенты/Проект`, не смешивая с цветными тегами Flit. | Расширение; P2/M | [Tag model](D:/ToDelete/spacedrive/core/src/domain/tag.rs:1) |
| S13 | Удалять группы дублей по правилу «эти экземпляры сохранить», сверяя полные хеши непосредственно перед удалением. | Расширение Q06; P1/L | [удаление дублей](D:/ToDelete/spacedrive/core/src/ops/files/delete/duplicates.rs:326). Повторное чтение перед удалением у Spacedrive не гарантировано — для Flit это требование. |
| S14 | Разложить файлы по дате съёмки/изменения, году/месяцу/дню, типу или расширению с планом результата. | Новая; P2/M–L | [organize rules](D:/ToDelete/spacedrive/core/src/ops/files/organize/input.rs:10). Полезно для Загрузок и фотоархива. |
| S15 | Извлечение с `strip-components` и поддержка TAR.ZST. | Расширение; P2/M | [archive input](D:/ToDelete/spacedrive/core/src/ops/files/archive/input.rs:10) |
| S18 | Создать/обновить миниатюры для выбранной папки: отсутствующие, устаревшие или все, фоновым заданием. | Расширение; P2/M | [thumb job](D:/ToDelete/spacedrive/core/src/ops/thumbs/generate.rs:31) |
| S21 | Круговая карта занятого места (Sunburst) с переходом внутрь, на основе уже имеющихся размеров папок. | Расширение; P2/M | [analyzer](D:/ToDelete/spacedrive/packages/interface/src/routes/analyzer/index.tsx:82) |

## Дополнения второго прохода

| ID | Сценарий | Оценка | Исполнение у донора и отличие Flit |
|---|---|---|---|
| N03 | Восстанавливать после перезапуска вид, фильтр, сортировку и колонки отдельно для каждой вкладки. | Расширение; P2/M | E++: [снимок вкладки](D:/ToDelete/explorerplusplus/Explorer++/Explorer++/Tab.cpp:154). [FolderViews](D:/Projects/Flit/crates/flit-core/src/ui/details.rs:272) Flit привязаны к пути. |
| N04 | Полные действия над папкой в меню посещённых/родительских папок: меню оболочки, перетаскивание наружу, открытие в другой вкладке/окне; перетаскивание текущей папки из адресной строки. | Расширение; P2/S–M | E++: [ShellItemsMenu](D:/ToDelete/explorerplusplus/Explorer++/Explorer++/ShellItemsMenu.cpp:39), [drag address](D:/ToDelete/explorerplusplus/Explorer++/Explorer++/AddressBar.cpp:141) |
| N05 | Повторная F2 при переименовании по кругу выделяет имя без расширения, расширение и всё имя. | Расширение; P2/S | E++: [RenameStage](D:/ToDelete/explorerplusplus/Explorer++/Explorer++/LabelEditHandler.cpp:201). Flit: [rename](D:/Projects/Flit/crates/flit-core/src/ui/ops.rs:1641) |
| N06 | Копировать/перемещать дерево с рекурсивным фильтром по сохранённому шаблону поиска, сохраняя структуру. | Расширение Q07; P2/M | DC: [исключение узлов](D:/ToDelete/doublecmd/src/filesources/ufilesourcetreebuilder.pas:267). [FileOp::Copy/Move](D:/Projects/Flit/crates/flit-core/src/provider.rs:153) без фильтра. |
| N07 | Маска имени назначения прямо в copy/move: переименовать при переносе. С предпросмотром конфликтов и Undo. | Расширение; P2/M | DC: [RenameMask](D:/ToDelete/doublecmd/src/filesources/filesystem/ufilesystemutil.pas:1199) |
| N20 | Сортировка по нескольким колонкам с приоритетом и своим направлением: тип → дата по убыванию → имя. | Расширение; P1/M | DC: [Shift/Ctrl-click](D:/ToDelete/doublecmd/src/fileviews/ucolumnsfileview.pas:325). [SortSpec](D:/Projects/Flit/crates/flit-core/src/sort.rs:40) — одно поле. |
| N22 | Сортировать папки по имени независимо от сортировки файлов. «Папки первыми» уже есть. | Расширение; P2/S–M | DC: [folder comparator](D:/ToDelete/doublecmd/src/ufilesorting.pas:463) |
| N25 | Строка быстрого поиска с масками и переходами к следующему/предыдущему/первому/последнему совпадению, без скрытия строк. | Расширение; P2/M | DC: [SearchFile](D:/ToDelete/doublecmd/src/fileviews/uorderedfileview.pas:641). Переход по первым буквам уже есть. |
| N26 | Удалять выбранные элементы внутри архива. | Расширение; P2/M–L | DC: [WcxDeleteFiles](D:/ToDelete/doublecmd/src/filesources/wcxarchive/uwcxarchivedeleteoperation.pas:147). [ArchiveOp](D:/Projects/Flit/crates/flit-core/src/archive.rs:221) без удаления. |
| N27 | Открыть файл из архива во внешнем редакторе и после правки вернуть его в архив с подтверждением. | Расширение; P1–P2/L | DC: [copy-back](D:/ToDelete/doublecmd/src/ushowform.pas:493). Flit [извлекает для открытия](D:/Projects/Flit/crates/flit-archive/src/lib.rs:139), обратной записи нет. |
| N28 | Просмотр и поиск во всём большом текстовом/двоичном файле, включая конец. | Расширение X01; P1–P2/L | DC: [MapFile](D:/ToDelete/doublecmd/components/viewer/viewercontrol.pas:1727). Flit [читает начало](D:/Projects/Flit/crates/flit/src/preview.rs:85). |
| N29 | Следить за концом растущего лога и показывать новые записи. | Расширение; P2/M–L после N28 | DC: [GoEnd](D:/ToDelete/doublecmd/src/fviewer.pas:3959). Flit [перезагружает превью](D:/Projects/Flit/crates/flit-core/src/ui/preview_pane.rs:155), но с начала. |
| N32 | Свои действия «открыть/просмотреть/править» по расширению, независимо от системных ассоциаций. | Расширение; P2/M | DC: [file associations](D:/ToDelete/doublecmd/src/frames/foptionsfileassoc.pas:503). «Открыть с помощью» уже есть. |

## Что брать в первую очередь

| Порядок | Пакет | Почему | Где во Flit |
|---|---|---|---|
| 1 | Переименование: R01/R03/R05/R06/R09/R10, малое N05 | Форма, предпросмотр, проверка и Undo уже есть | [batch_rename.rs](D:/Projects/Flit/crates/flit-core/src/ui/batch_rename.rs:225) |
| 2 | Сортировка и быстрый поиск: N20/N25 | Пробелы в ежедневной работе | SortSpec, заголовки колонок, переход по совпадениям |
| 3 | Выделение R12–R14; экспорт U06/U07 | Небольшие правки, полезны в каждой папке | Actions, выделение, контекстное меню |
| 4 | Вкладки и закладки: U01/U02/U04/N03, затем N04 | Устойчивые рабочие места и точное восстановление | Tab/tabbar, сеанс, боковая панель |
| 5 | Архивы и хеши: O12/A01/A02/N26, затем N27 | Используют готовые хеши, архивы и задания | Архивный диалог и backend |
| 6 | Сравнение панелей: C01/C05 | Сильная сторона Flit — мультипанели; база для синхронизации | Panels и отдельная модель сравнения |
| 7 | Коллекции: Q10–Q12 | Существующие операции над файлами из разных папок | Общая модель коллекции, поиск, боковая панель |
| 8 | Дубликаты: Q06/S13 | Уборка больших дисков и архивов | Поиск + поэтапное хеширование + сверка перед удалением |
| 9 | Просмотр больших файлов: X01/N28/N29 | Снимают ограничение превью для текстов и логов | Отдельная модель чтения окнами, без I/O в UI-потоке |
| 10 | Поиск по содержимому и пресеты: Q01/Q07–Q09 | Сценарии вне системного индекса | flit-search, поиск в UI |
| 11 | Очередь: O01/O02, затем N06/N07 | Предсказуемость длинных операций | jobs.rs, OpCtl, движки |
| 12 | Синхронизация папок: C02/C03 | Сильная функция, отдельный проект с планом действий | Новая модель плана в flit-core, исполнение через FileOp |
| 13 | Докопирование с места обрыва: O05 | Только по запросу, без записи на диск во время обычных операций | Движки копирования |

## Убрано из списка 15.11.2026

| Причина | Пункты |
|---|---|
| Уже делается: сортировка и новые колонки свойств оболочки (rest2, W9), ссылки на Windows (rest2, W10), FTP на Windows (todo №15) | N02, U10, X10; часть U11 |
| Отказались: облака вне рамок; AppUserModelID не ставим (W17); подсказка пакета вместо установки | S30, U18, S24 |
| Инфраструктура и продуктовые области Spacedrive, не нужные файловому менеджеру: переносимый каталог и всё на нём, P2P, репликация, удалённые операции, единый byte provider, loopback-монтирование, адаптеры источников, WASM-расширения, MCP/API, менеджер сервисов | S01–S04, S06–S10, S16, S20, S22, S25–S29, S31–S33, N09–N15 |
| Встроенные приложения вместо внешних: текстовый и графический редактор, скриншоты, книжный режим, кадры GIF/ICO, слайд-шоу, встроенный терминал и отдельная командная строка, text и binary diff | X02–X05, N16, N17, N30, N31, C06, C07 |
| Скриптинг, плагины и профили внешних архиваторов | X08, X11, A05, A06, Q13 |
| Постоянная нагрузка, несовместимая с быстротой Flit: журнал эффектов и checkpoint пишут на диск в каждой операции; отдельное дерево держит второй обход и наблюдатели; правила цветов и список игнорируемых масок проверяются для каждой строки; фильтр без диакритики держит копию всех имён; проверка после копирования удваивает чтение | S11, S12, N01, U13, U16, N24, O04; колонки «Владелец» и «Число жёстких ссылок» на Windows (часть U11) |
| Узкие или малополезные настройки и функции | R11, C04, Q02, Q05, O03, O07–O11, O13, U05, U12, U14, U15, U17, S17, S19, S23, N08, N18, N19, N21, N23; затирание файлов перед удалением |

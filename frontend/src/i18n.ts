// Локализация интерфейса без переписывания ~2500 шаблонных строк: русские
// строки остаются ключами в разметке, при locale='en' словарь подменяет текст
// прямо в DOM. MutationObserver доставляет записи микротаском — до кадра,
// поэтому «вспышки» русского перед переводом нет. Перенос разметки на t() —
// отдельная волна; этот слой даёт английский интерфейс уже сейчас и не
// рискует логикой шаблонов.
import { ref } from 'vue'

export type Locale = 'ru' | 'en'

export const locale = ref<Locale>('ru')

const EN: Record<string, string> = {
  // Навигация и заголовки страниц
  'Главная': 'Home', 'Играть': 'Play', 'Экземпляры': 'Instances', 'Аккаунты': 'Accounts',
  'Каталог': 'Catalog', 'Моды': 'Mods', 'Модпаки': 'Modpacks', 'Ресурспаки': 'Resource packs',
  'Шейдеры': 'Shaders', 'Датапаки': 'Data packs', 'Плагины': 'Plugins', 'Настройки': 'Settings',
  'Консоль': 'Console', 'Миры': 'Worlds', 'Серверы': 'Servers', 'Скриншоты': 'Screenshots',
  'Снапшоты': 'Snapshots', 'Файлы': 'Files', 'Обзор': 'Overview', 'Профиль': 'Profile',
  'Инструменты': 'Tools', 'Студия тем': 'Theme studio', 'Самообновление': 'Self-update',
  'Рабочая область': 'Workspace', 'Библиотека игр': 'Game library', 'Профили доступа': 'Access profiles',
  'Моды и ресурсы': 'Mods and resources', 'Готовые сборки': 'Ready-made packs',
  'Расширения интерфейса': 'Interface extensions', 'Локальная сеть': 'Local network',
  'Параметры лаунчера': 'Launcher options', 'Студия оформления': 'Appearance studio',
  'Центр настройки тем': 'Theme center',
  'Плагины для лаунчера': 'Launcher plugins', 'Разделы экземпляра': 'Instance sections',
  'Общие настройки': 'General settings', 'Базовая настройка': 'Basic setup',

  // Общие кнопки и действия
  'Сохранить': 'Save', 'Сохранить изменения': 'Save changes', 'Сохраняю…': 'Saving…',
  'Отмена': 'Cancel', 'Отменить': 'Undo', 'Повторить': 'Redo', 'Закрыть': 'Close',
  'Удалить': 'Delete', 'Убрать': 'Remove', 'Создать': 'Create', 'Изменить': 'Edit',
  'Применить': 'Apply', 'Установить': 'Install', 'Переустановить': 'Reinstall',
  'Обновить': 'Update', 'Обновляю…': 'Updating…', 'Проверить': 'Check', 'Отправить': 'Send',
  'Загрузить': 'Upload', 'Скачать': 'Download', 'Импортировать': 'Import', 'Импорт': 'Import',
  'Экспорт': 'Export', 'Выбрать': 'Choose', 'Назад': 'Back', 'Далее': 'Next',
  'Готово': 'Done', 'Продолжить': 'Continue', 'Запустить': 'Launch', 'Остановить': 'Stop',
  'Останавливаю…': 'Stopping…', 'Перезапустить': 'Restart', 'Открыть': 'Open',
  'Копировать': 'Copy', 'Скопировано': 'Copied', 'Поиск': 'Search', 'Искать': 'Search',
  'Сбросить': 'Reset', 'Сбросить оформление': 'Reset appearance', 'Управлять': 'Manage',
  'Добавить': 'Add', 'Обновить данные': 'Refresh data', 'Новый экземпляр': 'New instance',
  'Создать экземпляр': 'Create instance', 'Показать ещё': 'Show more', 'Загружаю…': 'Loading…',
  'Повторить сохранение': 'Retry save', 'Всё сохранено': 'All saved', 'Выберите значение': 'Pick a value',
  'Все версии': 'All versions', 'По умолчанию': 'Default', 'Автоматически': 'Automatic',
  'Вверх': 'Up', 'Запись': 'Recording', 'Иконка': 'Icon', 'Баннер': 'Banner',
  'Папка': 'Folder', 'Новая папка': 'New folder', 'Путь к папке': 'Folder path',
  'Версия': 'Version', 'Версии': 'Versions', 'Редакция': 'Build',

  // Состояния и статусы
  'Изменения сохранены': 'Changes saved', 'Есть несохранённые изменения': 'There are unsaved changes',
  'Изменения применяются после сохранения': 'Changes apply after saving',
  'Версия не установлена': 'Version is not installed', 'Не определён': 'Unknown',
  'Работает': 'Running', 'Не работает': 'Not running', 'установлен.': 'installed.',
  'Готово к запуску.': 'Ready to launch.', 'Недоступно': 'Unavailable', 'недоступен': 'unavailable',
  'неизвестен': 'unknown', 'Пусто': 'Empty', 'Загружаю': 'Loading', 'Ожидание': 'Waiting',
  'Установлена последняя доступная версия.': 'The latest available version is installed.',
  'Обновлений нет': 'No updates', 'Файл сохранён': 'File saved', 'Снапшот создан': 'Snapshot created',
  'Снапшот восстановлен': 'Snapshot restored', 'Настройки сохранены': 'Settings saved',
  'Импорт отменён; файлы экземпляра не изменены.': 'Import canceled; instance files were not changed.',
  'Вход отменён.': 'Sign-in canceled.', 'Игра завершила работу': 'The game exited cleanly',
  'Вход выполнен': 'Signed in', 'Выход выполнен': 'Signed out',

  // Единицы и время
  'мин': 'min', 'ч': 'h', 'сек': 'sec', 'МБ': 'MB', 'ГБ': 'GB', 'КБ': 'KB', 'байт': 'bytes',
  'байта': 'bytes', 'модов': 'mods', 'файлов': 'files', 'символов': 'characters',

  // Настройки: разделы и поля
  'Внешний вид': 'Appearance', 'Поведение': 'Behavior', 'Запуск и управление': 'Launch and control',
  'Интеграции': 'Integrations', 'Сторонние источники': 'Third-party sources',
  'Система': 'System', 'Безопасность': 'Security', 'Сеть': 'Network', 'О лаунчере': 'About',
  'Заставка': 'Idle screen', 'Заставка простоя': 'Idle screen', 'Язык интерфейса': 'Interface language',
  'Источник входа': 'Sign-in source', 'Источник данных': 'Data source',
  'Режим разработчика': 'Developer mode', 'Каталоги ресурсов': 'Resource catalogs',
  'Каталоги плагинов серверов': 'Server plugin catalogs', 'Активный каталог': 'Active catalog',
  'Добавить каталог': 'Add catalog', 'Название': 'Name', 'Тип': 'Type',
  'только HTTPS': 'HTTPS only', 'Версии и metadata': 'Versions and metadata',
  'Показать сейчас': 'Show now', 'Скорость': 'Speed', 'Таймаут': 'Timeout',
  'Показывать заставку': 'Show idle screen', 'Включить': 'Enable', 'Выключить': 'Disable',

  // Студия тем
  'Тема': 'Theme', 'Цвета': 'Colors', 'Формы': 'Shapes', 'Обои': 'Wallpaper',
  'Навигация': 'Navigation', 'Движение': 'Motion', 'Страницы': 'Pages',
  'Режим, акцент, шрифт, масштаб': 'Mode, accent, font, scale',
  'Вся палитра лаунчера': 'The full launcher palette',
  'Закруглённости и плотность': 'Corner radii and density',
  'Картинка, размытие, видимость': 'Image, blur, visibility',
  'Сайдбар: положение и режим': 'Sidebar: position and mode',
  'Анимации страниц и поверхностей': 'Page and surface animations',
  'Темы и раскладки страниц': 'Page themes and layouts',
  'JSON палитры': 'JSON palette', 'Тёмная': 'Dark', 'Светлая': 'Light',
  'Плотность': 'Density', 'Контент': 'Content', 'Положение': 'Position',
  'Режим содержимого': 'Content mode', 'Плавающий режим': 'Floating mode',
  'Сайдбар не приклеен к краю окна': 'The sidebar is not glued to the window edge',
  'Размытие': 'Blur', 'Видимость': 'Visibility', 'Гарнитура': 'Typeface',
  'Шрифт интерфейса': 'Interface font', 'Загрузить баннер': 'Upload banner',
  'Элемент интерфейса': 'Interface element', 'Вернуть стандартный цвет': 'Restore the default color',
  'Закруглённость кнопки': 'Button corner radius', 'Закруглённость карточки': 'Card corner radius',
  'Закруглённость чипа': 'Chip corner radius', 'Все настройки навигации': 'All navigation settings',
  'Поведение сайдбара': 'Sidebar behavior', 'Метеорный поток': 'Meteor shower',
  'Акварельные разводы': 'Watercolor wash', 'Своя заставка': 'Custom screen',

  // Диалоги
  'Подтверждение': 'Confirmation', 'Удалить безвозвратно?': 'Delete permanently?',
  'Это действие нельзя отменить.': 'This action cannot be undone.',
  'Сохранить и продолжить': 'Save and continue', 'Не сохранять': 'Don’t save',
  'Свой каталог': 'Custom catalog', 'Официальный каталог': 'Official catalog',
  'Укажите имя экземпляра': 'Enter an instance name', 'Имя сохранения': 'Save name',

  // Аккаунты
  'Войти через Microsoft': 'Sign in with Microsoft', 'Выйти': 'Sign out',
  'Microsoft аккаунт': 'Microsoft account', 'Оффлайн-аккаунт': 'Offline account',
  'Добавить аккаунт': 'Add account', 'Активный аккаунт': 'Active account',

  // Bedrock-воркспейс: консоль, скриншоты, снапшоты
  'Консоль сессии': 'Session console', 'Архив логов': 'Log archive',
  'Скриншоты Bedrock': 'Bedrock screenshots', 'Снапшоты контента': 'Content snapshots',
  'Создать снапшот': 'Create snapshot', 'Остановлена': 'Stopped',
  'Паки ресурсов': 'Resource packs', 'Аддоны': 'Add-ons',
  'Резервные копии паков, аддонов и миров перед обновлениями контента.': 'Backups of packs, add-ons and worlds before content updates.',
  'Снимки внутриигровой камеры игры (com.mojang/minecraftScreenshots).': 'In-game camera shots (com.mojang/minecraftScreenshots).',
  'Живой поток вывода игры Bedrock и архив прошлых сессий.': 'Live Bedrock game output and past session archives.',
  'Лог появится после запуска игры.': 'The log will appear after the game starts.',

  // Активность: остановка и копирование
  'Копировать содержимое': 'Copy contents',
  'Остановка запрошена — операция завершит текущий шаг сама.': 'Stop requested — the operation will finish its current step on its own.',
  'Переименовать группу (экземпляры остаются в ней)': 'Rename the group (instances stay in it)',

  // Прочее частое
  'Неизвестная ошибка': 'Unknown error', 'Ошибка': 'Error', 'Информация': 'Information',
  'Подробнее': 'Details', 'Открыть настройки': 'Open settings', 'Фокус на поиск': 'Focus search',
  'Показать активность': 'Show activity', 'Сначала новые': 'Newest first',
  'По релевантности': 'By relevance', 'Больше загрузок': 'Most downloads',
  'Недавно обновлялись': 'Recently updated', 'Релизы': 'Releases',
  'Тестовые сборки': 'Test builds', 'Официальный каталог ядра': 'Official core catalog',

  // Экземпляры и профили запуска
  'Версия Minecraft': 'Minecraft version', 'Память JVM': 'JVM memory', 'Ширина окна': 'Window width',
  'Высота окна': 'Window height', 'по одному на строку': 'one per line', 'Восстановить': 'Restore',
  'Библиотека': 'Library', 'Отображаемое название': 'Display name', 'Порт': 'Port',
  'К последней строке': 'Jump to the last line', 'Диск': 'Disk', 'График CPU': 'CPU chart',
  'График RAM': 'RAM chart', 'Новый профиль': 'New profile', 'Имя экземпляра': 'Instance name',
  'Источник ядра': 'Core source', 'Ядро': 'Core', 'Выбрать JAR': 'Choose JAR',
  'Я принимаю EULA Minecraft': 'I accept the Minecraft EULA', 'Профили': 'Profiles',
  'Локальный профиль': 'Local profile', 'Среда запуска': 'Runtime environment',
  'Параллельные загрузки': 'Parallel downloads', 'Интервал, часов': 'Interval, hours',
  'Хранить копий': 'Copies to keep', 'Полноэкранный режим': 'Fullscreen mode',
  'Дополнительные JVM-аргументы': 'Extra JVM arguments', 'Повторные попытки загрузки': 'Download retries',
  'FPS записи': 'Recording FPS', 'Системные метрики': 'System metrics',
  'Например, Survival': 'For example, Survival', 'Порт сервера': 'Server port',
  'Количество копий': 'Number of copies', 'Консоль в реальном времени': 'Live console',
  'Не удалось обновить данные': 'Failed to refresh data', 'Быстро подключиться': 'Quick connect',
  'Файлы экземпляра': 'Instance files', 'Читаю папку…': 'Reading folder…',
  '· текстовый файл': '· text file', '· есть несохранённые изменения': '· there are unsaved changes',
  'Зависимости': 'Dependencies', 'Снапшоты ресурсов': 'Resource snapshots',
  'Группа': 'Group', 'Описание': 'Description', 'Дерево зависимостей модов': 'Mod dependency tree',
  'Закрыть редактор': 'Close editor', 'Содержимое файла': 'File contents',
  'Закрыть предпросмотр': 'Close preview', 'Удалить снапшот': 'Delete snapshot',
  'Выдержка лога': 'Log excerpt', 'Максимум игроков': 'Max players',
  'Максимум снапшотов': 'Max snapshots', 'Ник игрока': 'Player nickname',
  'Порт Minecraft': 'Minecraft port',
  'Консоль сервера': 'Server console', 'Команда сервера': 'Server command',
  'Память для Minecraft': 'Memory for Minecraft', 'Имя локального профиля': 'Local profile name',
  'Найти экземпляр': 'Find an instance',  'Оверлей': 'Overlay',
  'Профили игры и серверы Minecraft.': 'Game profiles and Minecraft servers.',
  'Новый сервер': 'New server', 'Загрузчик': 'Loader', 'Получаю список версий…': 'Fetching the version list…',
  'Нажмите «Обновить», чтобы получить список версий.': 'Press “Update” to fetch the version list.',
  'Системные требования': 'System requirements', 'Имя сервера': 'Server name',
  'Официальное': 'Official', 'Собственный JAR': 'Custom JAR', 'Профиль игры': 'Game profile',
  'К экземплярам': 'Go to instances', 'Экземпляр не выбран': 'No instance selected',
  'Вернитесь к списку экземпляров и откройте настройки нужного профиля.': 'Go back to the instance list and open the profile settings you need.',
  'Открыть экземпляры': 'Open instances', 'Подключённые': 'Connected',
  'по умолчанию': 'default', 'Настроить скин': 'Set up skin', 'Сделать по умолчанию': 'Make default',
  'Переместить выше': 'Move up', 'Переместить ниже': 'Move down', 'Удалить аккаунт': 'Delete account',
  'Аккаунтов пока нет': 'No accounts yet', 'Быстрое создание': 'Quick create',
  'Имя игрока': 'Player name', 'Создать offline-профиль': 'Create an offline profile',
  'Настройка скина': 'Skin settings', 'Загружаю скины и плащи из профиля Minecraft…': 'Loading skins and capes from the Minecraft profile…',
  'Модель рук': 'Arm model', 'Широкие': 'Wide', 'Стройные': 'Slim', 'Скины': 'Skins',
  'Текущий скин аккаунта и локальная библиотека.': 'The account’s current skin and the local library.',
  'Экипирован': 'Equipped', 'Плащи аккаунта': 'Account capes', 'Без плаща': 'No cape',
  'В профиле Minecraft пока нет доступных плащей.': 'There are no capes available in the Minecraft profile yet.',
  'Открыть папку со скинами': 'Open the skins folder', 'Импортировать файл скина': 'Import a skin file',
  'Сбросить скин': 'Reset skin', 'Ресурсы экземпляра': 'Instance resources',
  'Экземпляр для каталога': 'Instance for the catalog', 
  'Версия нового экземпляра': 'New instance version', 'Загрузчик нового экземпляра': 'New instance loader',
  'Vanilla не использует отдельную версию загрузчика.': 'Vanilla does not use a separate loader version.',
  'Импортировать архив': 'Import an archive', 'Декларативная вкладка': 'Declarative tab',
  'Вкладка создана из метаданных manifest. Файл entrypoint плагина не запускается.': 'The tab is built from manifest metadata. The plugin entrypoint file is not executed.',
  'Игра в локальной сети': 'Local network play',
  'Пути, память, источники и резервная копия файлов настраиваются здесь.': 'Paths, memory, sources and file backups are configured here.',
  'Обучение': 'Tutorial', 'Экскурсия по лаунчеру': 'A tour of the launcher',
  'Коротко покажем основные разделы и возможности. Экскурсию можно закрыть в любой момент.': 'A quick look at the main sections and features. You can close the tour at any moment.',
  'Повторить экскурсию': 'Restart the tour', 'Источники': 'Sources',
  'Обновить список': 'Refresh the list', 'Проверить Java': 'Check Java',
  'Профили настроек': 'Settings profiles', 'Импорт YML': 'YML import',
   'Использовать': 'Use', 'Графика': 'Graphics', 'Аккаунт': 'Account',
  'Добавляет --fullscreen к запуску.': 'Adds --fullscreen to the launch.',
  'Quick Play: мир': 'Quick Play: world', 'Quick Play: сервер': 'Quick Play: server',
  'JVM-пресет оптимизации': 'JVM optimization preset',
  'Дополнительные game-аргументы': 'Extra game arguments',
  'Выберите провайдеры и совместимые API endpoints. Изменения сохраняются локально.': 'Choose providers and compatible API endpoints. Changes are stored locally.',
  'база device-code и token endpoint': 'device-code and token endpoint base',
  'Вкл.': 'On', 
  'Экран при запуске': 'Screen on startup', 'Последний экран': 'Last screen',
  'Версии по умолчанию': 'Default versions', 'Loader по умолчанию': 'Default loader',
  'Аккаунт по умолчанию': 'Default account', 'Обновление metadata, часы': 'Metadata refresh, hours',
  'Timeout загрузки, секунд': 'Download timeout, seconds', 'Хранить логи, дней': 'Keep logs, days',
  '0 = без лимита': '0 = no limit', 'Запоминать экран': 'Remember the screen',
  'Возвращаться на последний открытый раздел': 'Return to the last opened section',
  'Открывать последний экземпляр': 'Open the last instance',
  'Сохранять рабочий контекст выбранного профиля': 'Keep the working context of the selected profile',
  'Запускать после установки': 'Open after install',
  'Автоматически открыть готовый экземпляр': 'Automatically open the finished instance',
  'Обновлять metadata автоматически': 'Refresh metadata automatically',
  'Проверять версии при открытии лаунчера': 'Check versions when the launcher opens',
  'Подтверждать опасные действия': 'Confirm dangerous actions',
  'Удаление экземпляров, ресурсов, плагинов и аккаунтов': 'Deleting instances, resources, plugins and accounts',
  'Показывать панель активности': 'Show the activity panel',
  'Логи, прогресс и отмена операций': 'Logs, progress and operation cancellation',
  'Показывать уведомления': 'Show notifications',
  'Короткие сообщения о результате действий': 'Short messages about action results',
  'Сворачивать при запуске': 'Minimize on launch',
  'Скрыть окно лаунчера после старта игры': 'Hide the launcher window after the game starts',
  'Телеметрия': 'Telemetry', 'Выключено по умолчанию': 'Off by default',
  'Горячие клавиши': 'Hotkeys', 'Игровой оверлей и запись': 'Game overlay and recording',
  'Оверлей в игре': 'In-game overlay', 'Виджет модов': 'Mods widget',
  'Список модов экземпляра в отдельной панели оверлея': 'The instance mod list in a separate overlay panel',
  'Виджет метрик': 'Metrics widget', 'CPU, RAM и диск игры в одном виджете': 'Game CPU, RAM and disk in one widget',
  'Записывать звук': 'Record audio', 'Захват системного звука через PulseAudio': 'System audio capture via PulseAudio',
  'Клавиша оверлея': 'Overlay hotkey', 'Клавиша записи': 'Recording hotkey',
  'Кодировщик': 'Encoder', 'Контейнер': 'Container', 'Качество': 'Quality',
  'CRF/CQ: меньше — лучше': 'CRF/CQ: lower is better',
  'Загрузка и по ядрам': 'Total and per-core load', 'Память и подкачка': 'Memory and swap',
  'NVIDIA через nvidia-smi; других нет — прячем честно': 'NVIDIA via nvidia-smi; others are absent — hidden honestly',
  'Диски': 'Disks', 'Заполнение смонтированных разделов': 'Mounted partition usage',
  'Температуры': 'Temperatures',  
}

// Динамические строки с подстановками: регулярка по русской фразе,
// английский шаблон с $1….
const PATTERNS: Array<[RegExp, string]> = [
  [/^Доступна версия (.*)\.$/, 'Version $1 is available.'],
  [/^Найдена версия (.*)$/, 'Version $1 found'],
  [/^Скачано (\d+) из (\d+) байт\.$/, 'Downloaded $1 of $2 bytes.'],
  [/^Код выхода: (.*)$/, 'Exit code: $1'],
  [/^Экземпляр не найден: (.*)$/, 'Instance not found: $1'],
  [/^Вкладка плагина (.*)$/, 'Plugin tab $1'],
  [/^Заставка «(.*)» добавлена$/, 'Idle screen “$1” added'],
  [/^Каталог (.*)$/, 'Catalog $1'],
  [/^Читаю папку (.*)$/, 'Reading folder $1'],
]

const SKIP_TAGS = new Set(['SCRIPT', 'STYLE', 'CODE', 'PRE', 'TEXTAREA', 'NOSCRIPT'])
const TRANSLATED_ATTRS = ['placeholder', 'title', 'aria-label', 'alt']

const originalText = new WeakMap<Text, string>()
const originalAttrs = new WeakMap<Element, Record<string, string>>()

let observer: MutationObserver | null = null
let applying = false

export function translate(text: string): string {
  const key = text.trim()
  if (!key) return text
  const exact = EN[key]
  if (exact !== undefined) return text.replace(key, exact)
  for (const [pattern, template] of PATTERNS) {
    const match = key.match(pattern)
    if (match) return text.replace(key, template.replace(/\$(\d)/g, (_, i) => match[Number(i)] ?? ''))
  }
  return text
}

function applyToTextNode(node: Text): void {
  if (applying || !node.nodeValue || !node.nodeValue.trim()) return
  const parent = node.parentElement
  if (parent && SKIP_TAGS.has(parent.tagName)) return
  const original = originalText.get(node)
  const source = original ?? node.nodeValue
  const translated = translate(source)
  if (translated === node.nodeValue) return
  originalText.set(node, source)
  applying = true
  node.nodeValue = translated
  applying = false
}

function applyToElement(element: Element): void {
  if (applying || SKIP_TAGS.has(element.tagName)) return
  for (const attr of TRANSLATED_ATTRS) {
    const value = element.getAttribute(attr)
    if (!value || !value.trim()) continue
    const stored = originalAttrs.get(element) ?? {}
    const source = stored[attr] ?? value
    const translated = translate(source)
    if (translated === value) continue
    stored[attr] = source
    originalAttrs.set(element, stored)
    applying = true
    element.setAttribute(attr, translated)
    applying = false
  }
}

function walk(root: Node): void {
  if (root.nodeType === Node.TEXT_NODE) {
    applyToTextNode(root as Text)
    return
  }
  if (root.nodeType !== Node.ELEMENT_NODE && root.nodeType !== Node.DOCUMENT_FRAGMENT_NODE) return
  const element = root as Element
  if (SKIP_TAGS.has(element.tagName)) return
  applyToElement(element)
  // FILTER_REJECT на служебных тегах режет их поддеревья целиком: переводы
  // не должны попадать в <code>/<pre> (консоль, JSON, логи).
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT, {
    acceptNode: (node) => node.nodeType === Node.ELEMENT_NODE && SKIP_TAGS.has((node as Element).tagName)
      ? NodeFilter.FILTER_REJECT
      : NodeFilter.FILTER_ACCEPT,
  })
  let current = walker.nextNode()
  while (current) {
    if (current.nodeType === Node.TEXT_NODE) applyToTextNode(current as Text)
    else applyToElement(current as Element)
    current = walker.nextNode()
  }
}

function restore(root: Node): void {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT)
  let current = walker.nextNode()
  applying = true
  try {
    while (current) {
      if (current.nodeType === Node.TEXT_NODE) {
        const node = current as Text
        const original = originalText.get(node)
        if (original !== undefined) node.nodeValue = original
      } else {
        const element = current as Element
        const stored = originalAttrs.get(element)
        if (stored) for (const [attr, value] of Object.entries(stored)) element.setAttribute(attr, value)
      }
      current = walker.nextNode()
    }
  } finally {
    applying = false
  }
}

export function setLocale(next: Locale): void {
  locale.value = next
  if (typeof document === 'undefined') return
  if (next === 'en') {
    walk(document.body)
    if (!observer) {
      observer = new MutationObserver((records) => {
        if (applying) return
        for (const record of records) {
          if (record.type === 'childList') {
            for (const node of record.addedNodes) walk(node)
          } else if (record.type === 'characterData' && record.target.nodeType === Node.TEXT_NODE) {
            applyToTextNode(record.target as Text)
          } else if (record.type === 'attributes' && record.target.nodeType === Node.ELEMENT_NODE) {
            applyToElement(record.target as Element)
          }
        }
      })
    }
    observer.observe(document.body, {
      childList: true, subtree: true, characterData: true,
      attributes: true, attributeFilter: TRANSLATED_ATTRS,
    })
  } else {
    observer?.disconnect()
    observer = null
    restore(document.body)
  }
}

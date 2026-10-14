#include <QApplication>
#include <QAbstractItemView>
#include <QComboBox>
#include <QCoreApplication>
#include <QCheckBox>
#include <QColor>
#include <QDialog>
#include <QEasingCurve>
#include <QAbstractAnimation>
#include <QDir>
#include <QFileDialog>
#include <QFileInfo>
#include <QFrame>
#include <QFormLayout>
#include <QEnterEvent>
#include <QGraphicsDropShadowEffect>
#include <QGraphicsOpacityEffect>
#include <QGridLayout>
#include <QGroupBox>
#include <QHBoxLayout>
#include <QInputDialog>
#include <QImage>
#include <QIcon>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLabel>
#include <QLinearGradient>
#include <QLineEdit>
#include <QListWidget>
#include <QMessageBox>
#include <QMouseEvent>
#include <QPlainTextEdit>
#include <QPainter>
#include <QPaintEvent>
#include <QPixmap>
#include <QProcess>
#include <QProcessEnvironment>
#include <QProgressBar>
#include <QPropertyAnimation>
#include <QPushButton>
#include <QRegularExpression>
#include <QScrollArea>
#include <QSize>
#include <QSizePolicy>
#include <QSpinBox>
#include <QStackedWidget>
#include <QStringList>
#include <QTimer>
#include <QVBoxLayout>
#include <QVector>
#include <QSvgRenderer>
#include <functional>
#include <utility>

static QIcon hugeIcon(const QString &content, int size = 24, const QColor &color = QColor(QStringLiteral("#c7d6f4"))) {
    const auto svg = QStringLiteral("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"24\" height=\"24\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"%1\" stroke-width=\"1.5\" stroke-linecap=\"round\" stroke-linejoin=\"round\">%2</svg>").arg(color.name(), content);
    QSvgRenderer renderer(svg.toUtf8());
    QPixmap pixmap(size, size);
    pixmap.fill(Qt::transparent);
    QPainter painter(&pixmap);
    renderer.render(&painter);
    return QIcon(pixmap);
}

[[maybe_unused]] static QIcon lucideIcon(const QString &content, int size = 19) {
    return hugeIcon(content, size);
}

[[maybe_unused]] static QIcon legacyLauncherIcon(const QString &name, int size = 19) {
    if (name == QStringLiteral("settings")) {
        return lucideIcon(QStringLiteral("<path d=\"M12 15.5a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7Z\"/><path d=\"M19.4 15a1.7 1.7 0 0 0 .34 1.88l.06.06-1.9 1.9-.06-.06a1.7 1.7 0 0 0-1.88-.34 1.7 1.7 0 0 0-1.03 1.56V22h-2.7v-.1a1.7 1.7 0 0 0-1.03-1.56 1.7 1.7 0 0 0-1.88.34l-.06.06-1.9-1.9.06-.06A1.7 1.7 0 0 0 7.76 17a1.7 1.7 0 0 0-1.56-1.03H6v-2.7h.1a1.7 1.7 0 0 0 1.56-1.03 1.7 1.7 0 0 0-.34-1.88l-.06-.06 1.9-1.9.06.06a1.7 1.7 0 0 0 1.88.34A1.7 1.7 0 0 0 12.13 7V6h2.7v1a1.7 1.7 0 0 0 1.03 1.56 1.7 1.7 0 0 0 1.88-.34l.06-.06 1.9 1.9-.06.06A1.7 1.7 0 0 0 19.3 12c.18.62.75 1.03 1.4 1.03h.3v2.7H20c-.28 0-.52-.05-.6-.73Z\"/>"), size);
    }
    if (name == QStringLiteral("play")) {
        return lucideIcon(QStringLiteral("<path d=\"m5 3 14 9-14 9V3Z\"/>"), size);
    }
    if (name == QStringLiteral("folder")) {
        return lucideIcon(QStringLiteral("<path d=\"M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z\"/>"), size);
    }
    if (name == QStringLiteral("download")) {
        return lucideIcon(QStringLiteral("<path d=\"M12 3v12\"/><path d=\"m7 10 5 5 5-5\"/><path d=\"M5 21h14\"/>"), size);
    }
    if (name == QStringLiteral("stop")) {
        return lucideIcon(QStringLiteral("<rect x=\"5\" y=\"5\" width=\"14\" height=\"14\" rx=\"2\"/>"), size);
    }
    if (name == QStringLiteral("refresh")) {
        return lucideIcon(QStringLiteral("<path d=\"M20 11a8.1 8.1 0 0 0-15.5-2M4 5v4h4\"/><path d=\"M4 13a8.1 8.1 0 0 0 15.5 2M20 19v-4h-4\"/>"), size);
    }
    if (name == QStringLiteral("trash")) {
        return lucideIcon(QStringLiteral("<path d=\"M3 6h18\"/><path d=\"M8 6V4h8v2\"/><path d=\"m19 6-1 14H6L5 6\"/><path d=\"M10 11v5M14 11v5\"/>"), size);
    }
    if (name == QStringLiteral("copy")) {
        return lucideIcon(QStringLiteral("<rect x=\"9\" y=\"9\" width=\"11\" height=\"11\" rx=\"2\"/><path d=\"M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1\"/>"), size);
    }
    if (name == QStringLiteral("skin")) {
        return lucideIcon(QStringLiteral("<circle cx=\"12\" cy=\"12\" r=\"9\"/><circle cx=\"8\" cy=\"10\" r=\"1\"/><circle cx=\"16\" cy=\"10\" r=\"1\"/><path d=\"M8 15c1.2 1 2.5 1.5 4 1.5s2.8-.5 4-1.5\"/>"), size);
    }
    return lucideIcon(QStringLiteral("<circle cx=\"12\" cy=\"12\" r=\"9\"/><path d=\"M12 8v4l3 2\"/>"), size);
}

static QIcon launcherIcon(const QString &name, int size = 24, const QColor &color = QColor(QStringLiteral("#c7d6f4"))) {
    if (name == QStringLiteral("home")) {
        return hugeIcon(QStringLiteral("<path d=\"M3 11.9896V14.5C3 17.7998 3 19.4497 4.02513 20.4749C5.05025 21.5 6.70017 21.5 10 21.5H14C17.2998 21.5 18.9497 21.5 19.9749 20.4749C21 19.4497 21 17.7998 21 14.5V11.9896C21 10.3083 21 9.46773 20.6441 8.74005C20.2882 8.01237 19.6247 7.49628 18.2976 6.46411L16.2976 4.90855C14.2331 3.30285 13.2009 2.5 12 2.5C10.7991 2.5 9.76689 3.30285 7.70242 4.90855L5.70241 6.46411C4.37533 7.49628 3.71179 8.01237 3.3559 8.74005C3 9.46773 3 10.3083 3 11.9896Z\"/>"), size, color);
    }
    if (name == QStringLiteral("instances")) {
        return hugeIcon(QStringLiteral("<path d=\"M12 22C11.1818 22 10.4002 21.6698 8.83693 21.0095C4.94564 19.3657 3 18.5438 3 17.1613V7M12 22C12.8182 22 13.5998 21.6698 15.1631 21.0095C19.0544 19.3657 21 18.5438 21 17.1613V7M12 22V11.3548\"/><path d=\"M8.32592 9.69138L5.40472 8.27785C3.80157 7.5021 3 7.11423 3 6.5C3 5.88577 3.80157 5.4979 5.40472 4.72215L8.32592 3.30862C10.1288 2.43621 11.0303 2 12 2C12.9697 2 13.8712 2.4362 15.6741 3.30862L18.5953 4.72215C20.1984 5.4979 21 5.88577 21 6.5C21 7.11423 20.1984 7.5021 18.5953 8.27785L15.6741 9.69138C13.8712 10.5638 12.9697 11 12 11C11.0303 11 10.1288 10.5638 8.32592 9.69138Z\"/><path d=\"M6 12L8 13M17 4L7 9\"/>"), size, color);
    }
    if (name == QStringLiteral("accounts")) {
        return hugeIcon(QStringLiteral("<path d=\"M18.4995 20.5C18.2663 17.5685 15.8417 15.2477 12.808 15.0521L11.9995 15C11.7107 15.0076 11.4416 15.0178 11.1877 15.0298C8.18075 15.1723 5.7304 17.5974 5.49951 20.5\"/><path d=\"M15.2495 9.25C15.2495 11.0449 13.7944 12.5 11.9995 12.5C10.2046 12.5 8.74952 11.0449 8.74952 9.25C8.74952 7.45507 10.2046 6 11.9995 6C13.7944 6 15.2495 7.45507 15.2495 9.25Z\"/><path d=\"M5.50249 8.5C5.17908 7.99485 4.99158 7.39432 4.99158 6.75C4.99158 4.95507 6.44665 3.5 8.24157 3.5C8.68752 3.5 9.1125 3.58982 9.49939 3.75235M18.4963 8.5C18.8197 7.99485 19.0072 7.39432 19.0072 6.75C19.0072 4.95507 17.5521 3.5 15.7572 3.5C15.3113 3.5 14.8863 3.58982 14.4994 3.75235\"/><path d=\"M22.0007 17.9996C21.8208 15.7374 19.9995 13.5 17.9995 13M1.99927 17.9996C2.17923 15.7374 4.00049 13.5 6.00049 13\"/>"), size, color);
    }
    if (name == QStringLiteral("settings")) {
        return hugeIcon(QStringLiteral("<path d=\"M15.5 12C15.5 13.933 13.933 15.5 12 15.5C10.067 15.5 8.5 13.933 8.5 12C8.5 10.067 10.067 8.5 12 8.5C13.933 8.5 15.5 10.067 15.5 12Z\"/><path d=\"M21.011 14.0965C21.5329 13.9558 21.7939 13.8854 21.8969 13.7508C22 13.6163 22 13.3998 22 12.9669V11.0332C22 10.6003 22 10.3838 21.8969 10.2493C21.7938 10.1147 21.5329 10.0443 21.011 9.90358C19.0606 9.37759 17.8399 7.33851 18.3433 5.40087C18.4817 4.86799 18.5509 4.60156 18.4848 4.44529C18.4187 4.28902 18.2291 4.18134 17.8497 3.96596L16.125 2.98673C15.7528 2.77539 15.5667 2.66972 15.3997 2.69222C15.2326 2.71472 15.0442 2.90273 14.6672 3.27873C13.208 4.73448 10.7936 4.73442 9.33434 3.27864C8.95743 2.90263 8.76898 2.71463 8.60193 2.69212C8.43489 2.66962 8.24877 2.77529 7.87653 2.98663L6.15184 3.96587C5.77253 4.18123 5.58287 4.28891 5.51678 4.44515C5.45068 4.6014 5.51987 4.86787 5.65825 5.4008C6.16137 7.3385 4.93972 9.37763 2.98902 9.9036C2.46712 10.0443 2.20617 10.1147 2.10308 10.2492C2 10.3838 2 10.6003 2 11.0332V12.9669C2 13.3998 2 13.6163 2.10308 13.7508C2.20615 13.8854 2.46711 13.9558 2.98902 14.0965C4.9394 14.6225 6.16008 16.6616 5.65672 18.5992C5.51829 19.1321 5.44907 19.3985 5.51516 19.5548C5.58126 19.7111 5.77092 19.8188 6.15025 20.0341L7.87495 21.0134C8.24721 21.2247 8.43334 21.3304 8.6004 21.3079C8.76746 21.2854 8.95588 21.0973 9.33271 20.7213C10.7927 19.2644 13.2088 19.2643 14.6689 20.7212C15.0457 21.0973 15.2341 21.2853 15.4012 21.3078C15.5682 21.3303 15.7544 21.2246 16.1266 21.0133L17.8513 20.034C18.2307 19.8187 18.4204 19.711 18.4864 19.5547C18.5525 19.3984 18.4833 19.132 18.3448 18.5991C17.8412 16.6616 19.0609 14.6226 21.011 14.0965Z\"/>"), size, color);
    }
    if (name == QStringLiteral("play")) {
        return hugeIcon(QStringLiteral("<path d=\"M18.8906 12.846C18.5371 14.189 16.8667 15.138 13.5257 17.0361C10.296 18.8709 8.6812 19.7884 7.37983 19.4196C6.8418 19.2671 6.35159 18.9776 5.95624 18.5787C5 17.6139 5 15.7426 5 12C5 8.2574 5 6.3861 5.95624 5.42132C6.35159 5.02245 6.8418 4.73288 7.37983 4.58042C8.6812 4.21165 10.296 5.12907 13.5257 6.96393C16.8667 8.86197 18.5371 9.811 18.8906 11.154C19.0365 11.7084 19.0365 12.2916 18.8906 12.846Z\"/>"), size, color);
    }
    if (name == QStringLiteral("folder")) {
        return hugeIcon(QStringLiteral("<path d=\"M8 7H16.75C18.8567 7 19.91 7 20.6667 7.50559C20.9943 7.72447 21.2755 8.00572 21.4944 8.33329C22 9.08996 22 10.1433 22 12.25C22 15.7612 22 17.5167 21.1573 18.7779C20.7926 19.3238 20.3238 19.7926 19.7779 20.1573C18.5167 21 16.7612 21 13.25 21H12C7.28595 21 4.92893 21 3.46447 19.5355C2 18.0711 2 15.714 2 11V7.94427C2 6.1278 2 5.21956 2.38032 4.53806C2.65142 4.05227 3.05227 3.65142 3.53806 3.38032C4.21956 3 5.1278 3 6.94427 3C8.10802 3 8.6899 3 9.19926 3.19101C10.3622 3.62712 10.8418 4.68358 11.3666 5.73313L12 7\"/>"), size, color);
    }
    if (name == QStringLiteral("download")) {
        return hugeIcon(QStringLiteral("<path d=\"M2.99969 17.0002C2.99969 17.9302 2.99969 18.3952 3.10192 18.7767C3.37932 19.8119 4.18796 20.6206 5.22324 20.898C5.60474 21.0002 6.06972 21.0002 6.99969 21.0002L16.9997 21.0002C17.9297 21.0002 18.3947 21.0002 18.7762 20.898C19.8114 20.6206 20.6201 19.8119 20.8975 18.7767C20.9997 18.3952 20.9997 17.9302 20.9997 17.0002\"/><path d=\"M16.4998 11.5002C16.4998 11.5002 13.1856 16.0002 11.9997 16.0002C10.8139 16.0002 7.49976 11.5002 7.49976 11.5002M11.9997 15.0002V3.00016\"/>"), size, color);
    }
    if (name == QStringLiteral("stop")) {
        return hugeIcon(QStringLiteral("<path d=\"M4 12C4 8.72077 4 7.08116 4.81382 5.91891C5.1149 5.48891 5.48891 5.1149 5.91891 4.81382C7.08116 4 8.72077 4 12 4C15.2792 4 16.9188 4 18.0811 4.81382C18.5111 5.1149 18.8851 5.48891 19.1862 5.91891C20 7.08116 20 8.72077 20 12C20 15.2792 20 16.9188 19.1862 18.0811C18.8851 18.5111 18.5111 18.8851 18.0811 19.1862C16.9188 20 15.2792 20 12 20C8.72077 20 7.08116 20 5.91891 19.1862C5.48891 18.8851 5.1149 18.5111 4.81382 18.0811C4 16.9188 4 15.2792 4 12Z\"/>"), size, color);
    }
    if (name == QStringLiteral("refresh")) {
        return hugeIcon(QStringLiteral("<path d=\"M20.0092 2V5.13219C20.0092 5.42605 19.6418 5.55908 19.4537 5.33333C17.6226 3.2875 14.9617 2 12 2C6.47715 2 2 6.47715 2 12C2 17.5228 6.47715 22 12 22C17.5228 22 22 17.5228 22 12\"/>"), size, color);
    }
    if (name == QStringLiteral("trash")) {
        return hugeIcon(QStringLiteral("<path d=\"M19.5 5.5L18.8803 15.5251C18.7219 18.0864 18.6428 19.3671 18.0008 20.2879C17.6833 20.7431 17.2747 21.1273 16.8007 21.416C15.8421 22 14.559 22 11.9927 22C9.42312 22 8.1383 22 7.17905 21.4149C6.7048 21.1257 6.296 20.7408 5.97868 20.2848C5.33688 19.3626 5.25945 18.0801 5.10461 15.5152L4.5 5.5\"/><path d=\"M3 5.5H21M16.0557 5.5L15.3731 4.09173C14.9196 3.15626 14.6928 2.68852 14.3017 2.39681C14.215 2.3321 14.1231 2.27454 14.027 2.2247C13.5939 2 13.0741 2 12.0345 2C10.9688 2 10.436 2 9.99568 2.23412C9.8981 2.28601 9.80498 2.3459 9.71729 2.41317C9.32164 2.7167 9.10063 3.20155 8.65861 4.17126L8.05292 5.5M9.5 16.5L9.5 10.5M14.5 16.5L14.5 10.5\"/>"), size, color);
    }
    if (name == QStringLiteral("copy")) {
        return hugeIcon(QStringLiteral("<path d=\"M7.5 14.5C7.5 11.2002 7.5 9.55025 8.52513 8.52513C9.55025 7.5 11.2002 7.5 14.5 7.5C17.7998 7.5 19.4497 7.5 20.4749 8.52513C21.5 9.55025 21.5 11.2002 21.5 14.5C21.5 17.7998 21.5 19.4497 20.4749 20.4749C19.4497 21.5 17.7998 21.5 14.5 21.5C11.2002 21.5 9.55025 21.5 8.52513 20.4749C7.5 19.4497 7.5 17.7998 7.5 14.5Z\"/><path d=\"M7.5 16.5C6.10355 16.5 5.40533 16.5 4.84402 16.3036C3.83866 15.9518 3.0482 15.1613 2.69641 14.156C2.5 13.5947 2.5 12.8964 2.5 11.5V9.5C2.5 6.20017 2.5 4.55025 3.52513 3.52513C4.55025 2.5 6.20017 2.5 9.5 2.5H11.5C12.8964 2.5 13.5947 2.5 14.156 2.69641C15.1613 3.0482 15.9518 3.83866 16.3036 4.84402C16.5 5.40533 16.5 6.10355 16.5 7.5\"/>"), size, color);
    }
    if (name == QStringLiteral("skin")) {
        return hugeIcon(QStringLiteral("<path d=\"M14 4C14 5.10457 13.1046 6 12 6C10.8954 6 10 5.10457 10 4C10 2.89543 10.8954 2 12 2C13.1046 2 14 2.89543 14 4Z\"/><path d=\"M16 12.5C16 10.6144 16 9.67157 15.4142 9.08579C14.8284 8.5 13.8856 8.5 12 8.5C10.1144 8.5 9.17157 8.5 8.58579 9.08579C8 9.67157 8 10.6144 8 12.5V14C8 14.9428 8 15.4142 8.29289 15.7071C8.58579 16 9.05719 16 10 16V20C10 20.9428 10 21.4142 10.2929 21.7071C10.5858 22 11.0572 22 12 22C12.9428 22 13.4142 22 13.7071 21.7071C14 21.4142 14 20.9428 14 20V16C14.9428 16 15.4142 16 15.7071 15.7071C16 15.4142 16 14.9428 16 14V12.5Z\"/>"), size, color);
    }
    return hugeIcon(QStringLiteral("<circle cx=\"12\" cy=\"12\" r=\"9\"/><path d=\"M12 8v4l3 2\"/>"), size, color);
}

static QString loaderAssetPath(const QString &name) {
    const auto fileName = name + QStringLiteral(".png");
    const QStringList candidates{
        QDir(QDir::currentPath()).filePath(QStringLiteral("cpp-ui/assets/loaders/") + fileName),
        QDir(QCoreApplication::applicationDirPath()).filePath(QStringLiteral("../assets/loaders/") + fileName),
        QDir(QCoreApplication::applicationDirPath()).filePath(QStringLiteral("assets/loaders/") + fileName),
        QDir(QCoreApplication::applicationDirPath()).filePath(QStringLiteral("../../share/trial-launcher/loaders/") + fileName),
        QStringLiteral("/usr/share/trial-launcher/loaders/") + fileName,
    };
    for (const auto &candidate : candidates) {
        if (QFileInfo::exists(candidate)) {
            return candidate;
        }
    }
    return {};
}

static QIcon loaderBadgeIcon(const QString &name, int size = 26) {
    QString color = QStringLiteral("#6f89c5");
    if (name == QStringLiteral("fabric")) color = QStringLiteral("#d9c99f");
    if (name == QStringLiteral("forge")) color = QStringLiteral("#d9a56d");
    if (name == QStringLiteral("neoforge")) color = QStringLiteral("#3caf73");
    if (name == QStringLiteral("quilt")) color = QStringLiteral("#a84cff");
    const auto letter = name.left(1).toUpper();
    const auto svg = QStringLiteral("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"32\" height=\"32\" viewBox=\"0 0 32 32\"><rect x=\"1\" y=\"1\" width=\"30\" height=\"30\" rx=\"9\" fill=\"%1\"/><text x=\"16\" y=\"22\" text-anchor=\"middle\" font-family=\"sans-serif\" font-size=\"17\" font-weight=\"700\" fill=\"#101722\">%2</text></svg>").arg(color, letter);
    QSvgRenderer renderer(svg.toUtf8());
    QPixmap pixmap(size, size);
    pixmap.fill(Qt::transparent);
    QPainter painter(&pixmap);
    renderer.render(&painter);
    return QIcon(pixmap);
}

static QIcon loaderIcon(const QString &name, int size = 26) {
    const auto path = loaderAssetPath(name);
    if (!path.isEmpty()) {
        const QPixmap pixmap(path);
        if (!pixmap.isNull()) {
            return QIcon(pixmap);
        }
    }
    return loaderBadgeIcon(name, size);
}

class MotionButton final : public QPushButton {
public:
    explicit MotionButton(const QString &text, QWidget *parent = nullptr) : QPushButton(text, parent) { init(); }
    MotionButton(const QIcon &icon, const QString &text, QWidget *parent = nullptr) : QPushButton(icon, text, parent) { init(); }

protected:
    void enterEvent(QEnterEvent *event) override {
        animateShadow(18.0);
        QPushButton::enterEvent(event);
    }

    void leaveEvent(QEvent *event) override {
        animateShadow(0.0);
        QPushButton::leaveEvent(event);
    }

    void mousePressEvent(QMouseEvent *event) override {
        animateShadow(5.0);
        QPushButton::mousePressEvent(event);
    }

    void mouseReleaseEvent(QMouseEvent *event) override {
        QPushButton::mouseReleaseEvent(event);
        animateShadow(underMouse() ? 18.0 : 0.0);
    }

private:
    void init() {
        setCursor(Qt::PointingHandCursor);
        shadow_ = new QGraphicsDropShadowEffect(this);
        shadow_->setColor(QColor(75, 125, 255, 92));
        shadow_->setOffset(0, 3);
        shadow_->setBlurRadius(0);
        setGraphicsEffect(shadow_);
    }

    void animateShadow(qreal value) {
        if (!shadow_ || !isEnabled()) {
            return;
        }
        auto *animation = new QPropertyAnimation(shadow_, "blurRadius", this);
        animation->setDuration(150);
        animation->setStartValue(shadow_->blurRadius());
        animation->setEndValue(value);
        animation->setEasingCurve(QEasingCurve::OutCubic);
        animation->start(QAbstractAnimation::DeleteWhenStopped);
    }

    QGraphicsDropShadowEffect *shadow_{};
};

class SkinPreviewWidget final : public QWidget {
public:
    explicit SkinPreviewWidget(QWidget *parent = nullptr) : QWidget(parent) {
        setMinimumSize(280, 380);
        setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Expanding);
    }

    void setSkinImage(const QImage &image) {
        image_ = image;
        update();
    }

    void setSlim(bool slim) {
        slim_ = slim;
        update();
    }

protected:
    void paintEvent(QPaintEvent *) override {
        QPainter painter(this);
        painter.setRenderHint(QPainter::Antialiasing, true);
        QLinearGradient background(0, 0, width(), height());
        background.setColorAt(0, QColor(QStringLiteral("#243758")));
        background.setColorAt(1, QColor(QStringLiteral("#182131")));
        painter.fillRect(rect(), background);
        painter.setPen(Qt::NoPen);
        painter.setBrush(QColor(0, 0, 0, 75));
        painter.drawEllipse(QRectF(width() / 2.0 - 74, height() - 64, 148, 24));
        if (image_.isNull()) {
            painter.setPen(QColor(QStringLiteral("#aebbd3")));
            painter.drawText(rect(), Qt::AlignCenter, QStringLiteral("Выберите PNG-скин\n\n64×64 или 64×32"));
            return;
        }
        const int scale = qMax(4, qMin(width() / 15, height() / 24));
        const int center = width() / 2;
        const int head = 8 * scale;
        const int arm = (slim_ ? 3 : 4) * scale;
        const int body = 8 * scale;
        const int leg = 4 * scale;
        const int top = qMax(20, height() / 8);
        drawPart(painter, QRect(center - head / 2, top, head, head), QRect(8, 8, 8, 8));
        drawPart(painter, QRect(center - body / 2, top + head + 8, body, 12 * scale), QRect(20, 20, 8, 12));
        drawPart(painter, QRect(center - body / 2 - arm - 5, top + head + 8, arm, 12 * scale), QRect(44, 20, 4, 12));
        drawPart(painter, QRect(center + body / 2 + 5, top + head + 8, arm, 12 * scale), QRect(36, 52, 4, 12));
        drawPart(painter, QRect(center - body / 2, top + head + 12 * scale + 14, leg, 12 * scale), QRect(4, 20, 4, 12));
        drawPart(painter, QRect(center + 4, top + head + 12 * scale + 14, leg, 12 * scale), QRect(20, 52, 4, 12));
    }

private:
    void drawPart(QPainter &painter, const QRect &target, const QRect &source) {
        if (image_.width() < source.right() || image_.height() < source.bottom()) {
            painter.fillRect(target, QColor(QStringLiteral("#5c719d")));
            return;
        }
        painter.drawImage(target, image_, source);
    }

    QImage image_;
    bool slim_{false};
};

class ProgressDialog final : public QDialog {
public:
    explicit ProgressDialog(QWidget *parent) : QDialog(parent) {
        setWindowTitle(QStringLiteral("Nivora Launcher — установка"));
        setWindowModality(Qt::NonModal);
        setWindowFlag(Qt::WindowStaysOnTopHint, true);
        resize(760, 470);
        auto *layout = new QVBoxLayout(this);
        title_ = new QLabel(QStringLiteral("Подготовка..."));
        title_->setObjectName(QStringLiteral("progressTitle"));
        layout->addWidget(title_);
        source_ = new QLabel;
        source_->setWordWrap(true);
        source_->setObjectName(QStringLiteral("progressSource"));
        layout->addWidget(source_);
        bar_ = new QProgressBar;
        bar_->setRange(0, 100);
        bar_->setValue(0);
        layout->addWidget(bar_);
        log_ = new QPlainTextEdit;
        log_->setReadOnly(true);
        layout->addWidget(log_, 1);
        stop_ = new MotionButton(QStringLiteral("Остановить"));
        layout->addWidget(stop_, 0, Qt::AlignRight);
         connect(stop_, &QPushButton::clicked, this, [this] {
             stopping_ = true;
             stop_->setEnabled(false);
             title_->setText(QStringLiteral("Останавливаю..."));
             if (cancel_) {
                 cancel_();
             }
         });
         pulseTimer_ = new QTimer(this);
         connect(pulseTimer_, &QTimer::timeout, this, [this] {
             if (stopping_ || !source_->text().isEmpty()) {
                 return;
             }
             static const QStringList dots{QString(), QStringLiteral("."), QStringLiteral(".."), QStringLiteral("...")};
             title_->setText(QStringLiteral("Подготовка") + dots.at(pulseStep_++ % dots.size()));
         });
         pulseTimer_->start(420);
         setStyleSheet(QStringLiteral(
             "QDialog { background:#0e1521; color:#edf2ff; }"
             "QLabel { background:transparent; }"
             "QLabel#progressTitle { font-size:18px; font-weight:800; color:#ffffff; }"
             "QLabel#progressSource { color:#91a1bd; }"
             "QPlainTextEdit { background:#182437; border:1px solid #354a6b; border-radius:10px; padding:9px; }"
             "QProgressBar { background:#182437; border:1px solid #354a6b; border-radius:9px; text-align:center; min-height:17px; }"
             "QProgressBar::chunk { background:#668dff; border-radius:8px; }"
             "QPushButton { min-height:40px; background:#1c2b43; border:1px solid #3c5278; border-radius:10px; padding:0 16px; }"
             "QPushButton:hover { background:#2b4165; border-color:#79a0ed; }"
         ));
    }

    void setCancelHandler(std::function<void()> handler) { cancel_ = std::move(handler); }

    void appendLine(const QString &line) {
        if (!line.trimmed().isEmpty()) {
            log_->appendPlainText(line.trimmed());
            log_->ensureCursorVisible();
        }
    }

    void setProgress(int value) { bar_->setValue(qBound(0, value, 100)); }

    void setSource(const QString &value) {
        source_->setText(value);
        title_->setText(QStringLiteral("Загрузка..."));
    }

private:
    QLabel *title_{};
    QLabel *source_{};
    QProgressBar *bar_{};
    QPlainTextEdit *log_{};
    QPushButton *stop_{};
    std::function<void()> cancel_;
    QTimer *pulseTimer_{};
    int pulseStep_{0};
    bool stopping_{false};
};

class LauncherWindow final : public QWidget {
public:
    LauncherWindow() {
        setWindowTitle(QStringLiteral("Nivora Launcher"));
        setObjectName(QStringLiteral("launcherRoot"));
        resize(980, 720);
        setMinimumSize(760, 560);
        buildUi();
        refreshVersions();
    }

private:
    enum class Operation {
        None,
        Versions,
        LoaderVersions,
        Instances,
        Accounts,
        DebugAccount,
        MicrosoftLogin,
        Install,
        Launch,
        OpenFolder,
        SettingsLoad,
        SettingsSave,
        SkinsList,
        SkinsImport,
        SkinsApply,
        SkinsProfile,
        SkinsCapeApply,
    };

    struct Setting {
        QString key;
        QString value;
    };

    QComboBox *versionBox_{};
    QComboBox *loaderBox_{};
    QComboBox *accountBox_{};
    QComboBox *loaderVersion_{};
    QLineEdit *instanceName_{};
    QLineEdit *memory_{};
    QCheckBox *noAssets_{};
    QPlainTextEdit *log_{};
    QProgressBar *progress_{};
    QLabel *status_{};
    QVector<QPushButton *> buttons_;
    QProcess *process_{};
    ProgressDialog *progressDialog_{};
    QStackedWidget *pages_{};
    QListWidget *instanceList_{};
    QListWidget *accountList_{};
    QLabel *homeAccount_{};
    QLabel *homeInstance_{};
    QLabel *settingsPaths_{};
    QLabel *settingsStatus_{};
    QLineEdit *settingsGameDir_{};
    QLineEdit *settingsClientId_{};
    QLineEdit *settingsJavaPath_{};
    QLineEdit *settingsMemory_{};
    QSpinBox *settingsMaxDownloads_{};
    QVector<QPushButton *> navButtons_;
    QGraphicsOpacityEffect *pageEffect_{};
    QString skinAccountId_;
    QDialog *skinDialog_{};
    QListWidget *skinList_{};
    SkinPreviewWidget *skinPreview_{};
    QComboBox *skinModel_{};
    QComboBox *skinCape_{};
    bool settingsLoaded_{false};
    bool offlineAutoCreated_{false};
    Operation operation_{Operation::None};
    QString output_;
    QVector<Setting> pendingSettings_;
    int pendingSettingIndex_{0};

    void setPage(int index) {
        pages_->setCurrentIndex(index);
        for (int buttonIndex = 0; buttonIndex < navButtons_.size(); ++buttonIndex) {
            navButtons_.at(buttonIndex)->setChecked(buttonIndex == index);
        }
        if (!pageEffect_) {
            pageEffect_ = new QGraphicsOpacityEffect(pages_);
            pages_->setGraphicsEffect(pageEffect_);
        }
        auto *animation = new QPropertyAnimation(pageEffect_, "opacity", this);
        animation->setDuration(220);
        animation->setStartValue(0.76);
        animation->setEndValue(1.0);
        animation->setEasingCurve(QEasingCurve::OutCubic);
        animation->start(QAbstractAnimation::DeleteWhenStopped);
    }

    void buildUi() {
        auto *shell = new QHBoxLayout(this);
        shell->setContentsMargins(0, 0, 0, 0);
        shell->setSpacing(0);

        auto *sidebar = new QWidget;
        sidebar->setObjectName(QStringLiteral("sidebar"));
        sidebar->setMinimumWidth(210);
        sidebar->setMaximumWidth(240);
        auto *sidebarLayout = new QVBoxLayout(sidebar);
        sidebarLayout->setContentsMargins(16, 22, 16, 18);
        sidebarLayout->setSpacing(8);
        auto *brand = new QLabel(QStringLiteral("TRIAL\nLAUNCHER"));
        brand->setObjectName(QStringLiteral("brand"));
        sidebarLayout->addWidget(brand);
        auto *brandSubtitle = new QLabel(QStringLiteral("CachyOS beta"));
        brandSubtitle->setObjectName(QStringLiteral("sidebarMuted"));
        sidebarLayout->addWidget(brandSubtitle);
        sidebarLayout->addSpacing(24);

        pages_ = new QStackedWidget;
        auto *homePage = new QWidget;
        auto *instancePage = new QWidget;
        auto *accountPage = new QWidget;
        auto *settingsPage = new QWidget;
        pages_->addWidget(homePage);
        pages_->addWidget(instancePage);
        pages_->addWidget(accountPage);
        pages_->addWidget(settingsPage);
        shell->addWidget(sidebar);
        shell->addWidget(pages_, 1);

        auto addNavigation = [this, sidebarLayout](const QString &text, const QString &icon, int index) {
            auto *button = new MotionButton(launcherIcon(icon, 24), text);
            button->setObjectName(QStringLiteral("navButton"));
            button->setCheckable(true);
            button->setIconSize(QSize(24, 24));
            sidebarLayout->addWidget(button);
            navButtons_.append(button);
            button->setChecked(index == 0);
            connect(button, &QPushButton::clicked, this, [this, index] {
                setPage(index);
                if (index == 1) {
                    refreshInstances();
                } else if (index == 2) {
                    refreshAccounts();
                } else if (index == 3 && !settingsLoaded_) {
                    loadSettings();
                }
            });
        };
        addNavigation(QStringLiteral("Главная"), QStringLiteral("home"), 0);
        addNavigation(QStringLiteral("Экземпляры"), QStringLiteral("instances"), 1);
        addNavigation(QStringLiteral("Аккаунты"), QStringLiteral("accounts"), 2);
        addNavigation(QStringLiteral("Настройки"), QStringLiteral("settings"), 3);
        sidebarLayout->addStretch(1);
        auto *sidebarHint = new QLabel(QStringLiteral("GPLv3\nОфициальные источники Mojang"));
        sidebarHint->setObjectName(QStringLiteral("sidebarMuted"));
        sidebarLayout->addWidget(sidebarHint);

        auto *root = new QVBoxLayout(instancePage);
        root->setContentsMargins(24, 22, 24, 20);
        root->setSpacing(14);

        auto *title = new QLabel(QStringLiteral("Запуск игры"));
        title->setObjectName(QStringLiteral("title"));
        root->addWidget(title);
        auto *subtitle = new QLabel(QStringLiteral("Выберите сборку, настройте экземпляр и запускайте без лишних окон."));
        subtitle->setObjectName(QStringLiteral("subtitle"));
        root->addWidget(subtitle);

        auto *instanceStrip = new QGroupBox(QStringLiteral("Экземпляры"));
        auto *instanceStripLayout = new QHBoxLayout(instanceStrip);
        instanceList_ = new QListWidget;
        instanceList_->setMaximumHeight(92);
        instanceList_->setSelectionMode(QAbstractItemView::SingleSelection);
        instanceStripLayout->addWidget(instanceList_, 1);
        auto *instanceActions = new QVBoxLayout;
        auto *refreshInstancesButton = new MotionButton(launcherIcon(QStringLiteral("refresh")), QStringLiteral("Обновить"));
        auto *duplicateButton = new MotionButton(launcherIcon(QStringLiteral("copy")), QStringLiteral("Дублировать"));
        auto *deleteInstanceButton = new MotionButton(launcherIcon(QStringLiteral("trash")), QStringLiteral("Удалить"));
        instanceActions->addWidget(refreshInstancesButton);
        instanceActions->addWidget(duplicateButton);
        instanceActions->addWidget(deleteInstanceButton);
        instanceStripLayout->addLayout(instanceActions);
        root->addWidget(instanceStrip);

        auto *settings = new QGroupBox(QStringLiteral("Игра"));
        auto *form = new QGridLayout(settings);
        form->setHorizontalSpacing(12);
        form->setVerticalSpacing(10);
        form->addWidget(new QLabel(QStringLiteral("Версия Minecraft")), 0, 0);
        versionBox_ = new QComboBox;
        form->addWidget(versionBox_, 0, 1, 1, 2);
        auto *refresh = new MotionButton(launcherIcon(QStringLiteral("refresh")), QStringLiteral("Обновить"));
        form->addWidget(refresh, 0, 3);

        form->addWidget(new QLabel(QStringLiteral("Загрузчик")), 1, 0);
        loaderBox_ = new QComboBox;
        loaderBox_->setIconSize(QSize(28, 28));
        loaderBox_->addItem(loaderIcon(QStringLiteral("vanilla")), QStringLiteral("vanilla"), QStringLiteral("vanilla"));
        loaderBox_->addItem(loaderIcon(QStringLiteral("fabric")), QStringLiteral("fabric"), QStringLiteral("fabric"));
        loaderBox_->addItem(loaderIcon(QStringLiteral("quilt")), QStringLiteral("quilt"), QStringLiteral("quilt"));
        loaderBox_->addItem(loaderIcon(QStringLiteral("forge")), QStringLiteral("forge"), QStringLiteral("forge"));
        loaderBox_->addItem(loaderIcon(QStringLiteral("neoforge")), QStringLiteral("neoforge"), QStringLiteral("neoforge"));
        form->addWidget(loaderBox_, 1, 1);
        loaderVersion_ = new QComboBox;
        loaderVersion_->addItem(QStringLiteral("Версия загрузчика не выбрана"), QString());
        loaderVersion_->setEnabled(false);
        form->addWidget(loaderVersion_, 1, 2, 1, 2);

        form->addWidget(new QLabel(QStringLiteral("Instance")), 2, 0);
        instanceName_ = new QLineEdit;
        instanceName_->setPlaceholderText(QStringLiteral("автоматически из версии"));
        form->addWidget(instanceName_, 2, 1, 1, 3);

        form->addWidget(new QLabel(QStringLiteral("Аккаунт")), 3, 0);
        accountBox_ = new QComboBox;
        form->addWidget(accountBox_, 3, 1);
        auto *debug = new MotionButton(QStringLiteral("Создать debug"));
        auto *microsoft = new MotionButton(QStringLiteral("Microsoft"));
        auto *accountButtons = new QHBoxLayout;
        accountButtons->setContentsMargins(0, 0, 0, 0);
        accountButtons->addWidget(debug);
        accountButtons->addWidget(microsoft);
        form->addLayout(accountButtons, 3, 2, 1, 2);

        form->addWidget(new QLabel(QStringLiteral("Память JVM")), 4, 0);
        memory_ = new QLineEdit(QStringLiteral("2G"));
        memory_->setMaximumWidth(100);
        form->addWidget(memory_, 4, 1);
        noAssets_ = new QCheckBox(QStringLiteral("не скачивать assets"));
        form->addWidget(noAssets_, 4, 2, 1, 2, Qt::AlignRight);
        form->setColumnStretch(1, 1);
        form->setColumnStretch(2, 1);
        root->addWidget(settings);

        auto *actions = new QHBoxLayout;
        addAction(actions, QStringLiteral("Установить"), [this] { install(); }, true, QStringLiteral("download"));
        addAction(actions, QStringLiteral("Запустить"), [this] { launch(); }, true, QStringLiteral("play"));
        root->addLayout(actions);
        auto *tools = new QHBoxLayout;
        addAction(tools, QStringLiteral("Открыть папку"), [this] { openFolder(); }, false, QStringLiteral("folder"));
        addAction(tools, QStringLiteral("Mods"), [this] { openInstanceSubfolder(QStringLiteral("mods")); }, false);
        addAction(tools, QStringLiteral("Resources"), [this] { openInstanceSubfolder(QStringLiteral("resourcepacks")); }, false);
        addAction(tools, QStringLiteral("Shaders"), [this] { openInstanceSubfolder(QStringLiteral("shaderpacks")); }, false);
        addAction(tools, QStringLiteral("Экспорт ZIP"), [this] { exportInstance(); }, false);
        addAction(tools, QStringLiteral("Создать ярлык"), [this] { createInstanceShortcut(); }, false);
        addAction(tools, QStringLiteral("Остановить"), [this] { stopBackend(); }, false, QStringLiteral("stop"));
        addAction(tools, QStringLiteral("Настройки"), [this] { setPage(3); if (!settingsLoaded_) loadSettings(); }, false, QStringLiteral("settings"));
        root->addLayout(tools);

        progress_ = new QProgressBar;
        progress_->setRange(0, 100);
        progress_->setValue(0);
        progress_->setTextVisible(true);
        root->addWidget(progress_);

        log_ = new QPlainTextEdit;
        log_->setReadOnly(true);
        log_->setPlaceholderText(QStringLiteral("Здесь появится подробный лог установки и запуска..."));
        root->addWidget(log_, 1);

        status_ = new QLabel(QStringLiteral("Готово"));
        status_->setObjectName(QStringLiteral("status"));
        root->addWidget(status_);
        auto *license = new QLabel(QStringLiteral("Nivora Launcher contributors • GPLv3 • без гарантий"));
        license->setObjectName(QStringLiteral("license"));
        root->addWidget(license);

        connect(refresh, &QPushButton::clicked, this, [this] { refreshVersions(); });
        connect(versionBox_, &QComboBox::currentTextChanged, this, [this] { reloadLoaderVersions(); });
        connect(loaderBox_, &QComboBox::currentTextChanged, this, [this] { reloadLoaderVersions(); });
        connect(refreshInstancesButton, &QPushButton::clicked, this, [this] { refreshInstances(); });
        buttons_.append(duplicateButton);
        buttons_.append(deleteInstanceButton);
        connect(duplicateButton, &QPushButton::clicked, this, [this] { duplicateInstance(); });
        connect(deleteInstanceButton, &QPushButton::clicked, this, [this] { deleteInstance(); });
        connect(instanceList_, &QListWidget::currentItemChanged, this, [this](QListWidgetItem *current) {
            if (current) {
                instanceName_->setText(current->data(Qt::UserRole).toString());
                if (homeInstance_) {
                    homeInstance_->setText(QStringLiteral("Экземпляр: ") + current->text());
                }
            }
        });
        connect(debug, &QPushButton::clicked, this, [this] { createDebugAccount(); });
        connect(microsoft, &QPushButton::clicked, this, [this] { loginMicrosoft(); });
        setStyleSheet(QStringLiteral(
             "QWidget#launcherRoot { background:#0b1019; }"
             "QWidget { background:transparent; color:#edf2ff; font-family:'Noto Sans','DejaVu Sans',sans-serif; font-size:13px; }"
             "QLabel { background:transparent; }"
             "QFrame { background:transparent; }"
             "QGroupBox { background:#151f30; border:1px solid #2d3d58; border-radius:14px; margin-top:12px; padding:18px 14px 14px; }"
             "QGroupBox::title { subcontrol-origin:margin; left:14px; padding:0 7px; background:#151f30; color:#a8bfff; font-size:12px; font-weight:700; }"
             "QComboBox, QLineEdit, QPlainTextEdit, QSpinBox { background:#182437; border:1px solid #354a6b; border-radius:10px; padding:9px 11px; selection-background-color:#536fbd; }"
             "QComboBox:hover, QLineEdit:hover, QPlainTextEdit:hover, QSpinBox:hover { border-color:#6385c9; }"
             "QComboBox:focus, QLineEdit:focus, QPlainTextEdit:focus, QSpinBox:focus { border:1px solid #86a9ff; }"
             "QComboBox QAbstractItemView { background:#182437; border:1px solid #46608e; selection-background-color:#3b5d9f; padding:5px; }"
             "QComboBox::drop-down { width:30px; border:0; }"
             "QPushButton { min-height:40px; background:#1c2b43; border:1px solid #3c5278; border-radius:10px; padding:0 16px; color:#e9f0ff; }"
             "QPushButton:hover { background:#2b4165; border-color:#79a0ed; color:#ffffff; }"
             "QPushButton:pressed { background:#17253a; border-color:#a5bfff; padding-top:2px; }"
             "QPushButton:focus { border:1px solid #9bb6ff; }"
             "QPushButton:disabled { color:#65748d; background:#151d2b; border-color:#253249; }"
             "QProgressBar { background:#182437; border:1px solid #354a6b; border-radius:9px; text-align:center; min-height:17px; color:#eaf0ff; }"
             "QProgressBar::chunk { background:#668dff; border-radius:8px; }"
             "QScrollArea, QScrollArea > QWidget > QWidget { background:transparent; border:0; }"
              "QLabel#title { font-size:27px; font-weight:800; color:#ffffff; }"
              "QLabel#subtitle, QLabel#status, QLabel#license, QLabel#sidebarMuted { color:#91a1bd; }"
              "QLabel#sectionTitle { font-size:15px; font-weight:800; color:#dce7ff; }"
              "QLabel#eyebrow, QLabel#statLabel { color:#83a6ff; font-size:11px; font-weight:800; letter-spacing:1px; }"
              "QLabel#heroMeta { color:#c0cff0; font-size:13px; }"
              "QLabel#statValue { color:#ffffff; font-size:15px; font-weight:800; }"
              "QFrame#hero { background:#172746; border:1px solid #4166a2; border-radius:18px; }"
              "QFrame#statCard { background:#151f30; border:1px solid #2d405f; border-radius:14px; }"
              "QWidget#sidebar { background:#0e1521; border-right:1px solid #263650; }"
              "QLabel#brand { font-size:22px; font-weight:900; color:#ffffff; }"
              "QPushButton#navButton { min-height:46px; text-align:left; background:transparent; border:1px solid transparent; color:#9baac2; padding:0 14px; border-radius:12px; }"
              "QPushButton#navButton:hover { background:#182740; border-color:#2b4164; color:#ffffff; }"
              "QPushButton#navButton:checked { background:#243b63; border-color:#506fae; color:#ffffff; }"
              "QPushButton#primary { background:#5c7ff0; border-color:#88a5ff; font-weight:800; min-height:46px; }"
              "QPushButton#primary:hover { background:#7698ff; border-color:#b3c5ff; }"
              "QListWidget { background:#182437; border:1px solid #354a6b; border-radius:10px; padding:6px; }"
              "QListWidget::item { background:transparent; border-radius:8px; padding:9px; margin:2px 0; }"
              "QListWidget::item:hover { background:#223451; }"
              "QListWidget::item:selected { background:#304d80; color:#ffffff; }"
         ));
        buildHomePage(homePage);
        buildAccountsPage(accountPage);
        buildSettingsPage(settingsPage);
    }

    void buildHomePage(QWidget *page) {
        auto *layout = new QVBoxLayout(page);
        layout->setContentsMargins(34, 30, 34, 30);
        layout->setSpacing(18);
        auto *hero = new QFrame;
        hero->setObjectName(QStringLiteral("hero"));
        auto *heroLayout = new QVBoxLayout(hero);
        heroLayout->setContentsMargins(24, 22, 24, 22);
        auto *eyebrow = new QLabel(QStringLiteral("TRIAL LAUNCHER  /  BETA"));
        eyebrow->setObjectName(QStringLiteral("eyebrow"));
        heroLayout->addWidget(eyebrow);
        auto *title = new QLabel(QStringLiteral("Ваша игра начинается здесь"));
        title->setObjectName(QStringLiteral("title"));
        heroLayout->addWidget(title);
        auto *subtitle = new QLabel(QStringLiteral("Чистый запуск Minecraft без перегруженных меню. Выберите экземпляр и нажмите Play."));
        subtitle->setObjectName(QStringLiteral("subtitle"));
        subtitle->setWordWrap(true);
        heroLayout->addWidget(subtitle);
        homeInstance_ = new QLabel(QStringLiteral("Экземпляр: не выбран"));
        homeAccount_ = new QLabel(QStringLiteral("Аккаунт: загружается..."));
        homeInstance_->setObjectName(QStringLiteral("heroMeta"));
        homeAccount_->setObjectName(QStringLiteral("heroMeta"));
        auto *meta = new QHBoxLayout;
        meta->addWidget(homeInstance_);
        meta->addSpacing(18);
        meta->addWidget(homeAccount_);
        meta->addStretch(1);
        heroLayout->addLayout(meta);
        layout->addWidget(hero);

        auto *stats = new QHBoxLayout;
        addHomeStat(stats, QStringLiteral("Версии"), QStringLiteral("1.0 — 26.3"), QStringLiteral("официальный manifest"));
        addHomeStat(stats, QStringLiteral("Загрузчики"), QStringLiteral("Fabric · Forge"), QStringLiteral("и Quilt · NeoForge"));
        addHomeStat(stats, QStringLiteral("Аккаунты"), QStringLiteral("Offline + Microsoft"), QStringLiteral("локальные профили"));
        layout->addLayout(stats);

        auto *play = new MotionButton(launcherIcon(QStringLiteral("play"), 26, QColor(QStringLiteral("#ffffff"))), QStringLiteral("Запустить выбранный экземпляр"));
        play->setObjectName(QStringLiteral("primary"));
        play->setMinimumHeight(48);
        layout->addWidget(play);
        connect(play, &QPushButton::clicked, this, [this] {
            setPage(1);
            launch();
        });

        auto *quickTitle = new QLabel(QStringLiteral("Быстрые действия"));
        quickTitle->setObjectName(QStringLiteral("sectionTitle"));
        layout->addWidget(quickTitle);
        auto *quick = new QHBoxLayout;
        auto *accounts = new MotionButton(launcherIcon(QStringLiteral("accounts")), QStringLiteral("Аккаунты"));
        auto *instances = new MotionButton(launcherIcon(QStringLiteral("instances")), QStringLiteral("Экземпляры"));
        auto *settings = new MotionButton(launcherIcon(QStringLiteral("settings")), QStringLiteral("Настройки"));
        quick->addWidget(accounts);
        quick->addWidget(instances);
        quick->addWidget(settings);
        layout->addLayout(quick);
        layout->addStretch(1);
        connect(accounts, &QPushButton::clicked, this, [this] { setPage(2); refreshAccounts(); });
        connect(instances, &QPushButton::clicked, this, [this] { setPage(1); refreshInstances(); });
        connect(settings, &QPushButton::clicked, this, [this] { setPage(3); if (!settingsLoaded_) loadSettings(); });
    }

    void addHomeStat(QHBoxLayout *layout, const QString &label, const QString &value, const QString &hint) {
        auto *card = new QFrame;
        card->setObjectName(QStringLiteral("statCard"));
        auto *cardLayout = new QVBoxLayout(card);
        cardLayout->setContentsMargins(16, 13, 16, 13);
        auto *labelWidget = new QLabel(label.toUpper());
        labelWidget->setObjectName(QStringLiteral("statLabel"));
        cardLayout->addWidget(labelWidget);
        auto *valueWidget = new QLabel(value);
        valueWidget->setObjectName(QStringLiteral("statValue"));
        valueWidget->setWordWrap(true);
        cardLayout->addWidget(valueWidget);
        auto *hintWidget = new QLabel(hint);
        hintWidget->setObjectName(QStringLiteral("sidebarMuted"));
        cardLayout->addWidget(hintWidget);
        layout->addWidget(card, 1);
    }

    void buildAccountsPage(QWidget *page) {
        auto *layout = new QVBoxLayout(page);
        layout->setContentsMargins(34, 30, 34, 30);
        auto *title = new QLabel(QStringLiteral("Аккаунты"));
        title->setObjectName(QStringLiteral("title"));
        layout->addWidget(title);
        layout->addWidget(new QLabel(QStringLiteral("Автономные аккаунты работают только локально. Microsoft-аккаунты используют официальный вход.")));
        accountList_ = new QListWidget;
        layout->addWidget(accountList_, 1);
        auto *actions = new QHBoxLayout;
        auto *microsoft = new MotionButton(launcherIcon(QStringLiteral("accounts")), QStringLiteral("Войти через Microsoft"));
        auto *offline = new MotionButton(launcherIcon(QStringLiteral("accounts")), QStringLiteral("Добавить автономную"));
        auto *edit = new MotionButton(launcherIcon(QStringLiteral("settings")), QStringLiteral("Редактировать"));
        auto *skins = new MotionButton(launcherIcon(QStringLiteral("skin")), QStringLiteral("Управление скинами"));
        actions->addWidget(microsoft);
        actions->addWidget(offline);
        actions->addWidget(edit);
        actions->addWidget(skins);
        layout->addLayout(actions);
        connect(microsoft, &QPushButton::clicked, this, [this] { loginMicrosoft(); });
        connect(offline, &QPushButton::clicked, this, [this] { createOfflineAccount(); });
        connect(edit, &QPushButton::clicked, this, [this] { editSelectedAccount(); });
        connect(skins, &QPushButton::clicked, this, [this] { openSkinManager(); });
    }

    void buildSettingsPage(QWidget *page) {
        auto *pageLayout = new QVBoxLayout(page);
        pageLayout->setContentsMargins(34, 30, 34, 30);
        pageLayout->setSpacing(14);
        auto *title = new QLabel(QStringLiteral("Настройки"));
        title->setObjectName(QStringLiteral("title"));
        pageLayout->addWidget(title);
        pageLayout->addWidget(new QLabel(QStringLiteral("Все изменения сохраняются в локальный профиль лаунчера.")));

        auto *scroll = new QScrollArea;
        scroll->setWidgetResizable(true);
        scroll->setFrameShape(QFrame::NoFrame);
        auto *content = new QWidget;
        auto *layout = new QVBoxLayout(content);
        layout->setContentsMargins(2, 8, 8, 12);
        layout->setSpacing(14);

        auto *game = new QGroupBox(QStringLiteral("Игра и файлы"));
        auto *gameForm = new QFormLayout(game);
        settingsGameDir_ = new QLineEdit;
        auto *gameDirRow = new QHBoxLayout;
        gameDirRow->addWidget(settingsGameDir_, 1);
        auto *gameDirBrowse = new MotionButton(QStringLiteral("Обзор"));
        gameDirRow->addWidget(gameDirBrowse);
        gameForm->addRow(QStringLiteral("Папка игры"), gameDirRow);
        settingsMemory_ = new QLineEdit;
        settingsMemory_->setPlaceholderText(QStringLiteral("например 4G"));
        gameForm->addRow(QStringLiteral("Память JVM"), settingsMemory_);
        settingsMaxDownloads_ = new QSpinBox;
        settingsMaxDownloads_->setRange(1, 32);
        settingsMaxDownloads_->setSuffix(QStringLiteral(" параллельных загрузок"));
        gameForm->addRow(QStringLiteral("Скорость установки"), settingsMaxDownloads_);
        layout->addWidget(game);

        auto *java = new QGroupBox(QStringLiteral("Java"));
        auto *javaForm = new QFormLayout(java);
        settingsJavaPath_ = new QLineEdit;
        auto *javaRow = new QHBoxLayout;
        javaRow->addWidget(settingsJavaPath_, 1);
        auto *javaBrowse = new MotionButton(QStringLiteral("Обзор"));
        javaRow->addWidget(javaBrowse);
        javaForm->addRow(QStringLiteral("Java executable"), javaRow);
        javaForm->addRow(QStringLiteral("Автовыбор"), new QLabel(QStringLiteral("Если путь пустой, лаунчер подберёт runtime для версии Minecraft.")));
        layout->addWidget(java);

        auto *services = new QGroupBox(QStringLiteral("Аккаунты и службы"));
        auto *servicesForm = new QFormLayout(services);
        settingsClientId_ = new QLineEdit;
        settingsClientId_->setPlaceholderText(QStringLiteral("Microsoft OAuth client ID (один раз)"));
        servicesForm->addRow(QStringLiteral("Microsoft client ID"), settingsClientId_);
        auto *microsoftHint = new QLabel(QStringLiteral("Microsoft требует зарегистрированное OAuth-приложение. Лаунчер не может создать его без владельца Azure-аккаунта."));
        microsoftHint->setWordWrap(true);
        servicesForm->addRow(QStringLiteral("Важно"), microsoftHint);
        layout->addWidget(services);

        auto *paths = new QGroupBox(QStringLiteral("Расположение данных"));
        auto *pathsLayout = new QVBoxLayout(paths);
        settingsPaths_ = new QLabel;
        settingsPaths_->setObjectName(QStringLiteral("sidebarMuted"));
        settingsPaths_->setWordWrap(true);
        pathsLayout->addWidget(settingsPaths_);
        auto *showPathsButton = new MotionButton(QStringLiteral("Показать каталоги"));
        pathsLayout->addWidget(showPathsButton, 0, Qt::AlignLeft);
        layout->addWidget(paths);
        layout->addStretch(1);
        scroll->setWidget(content);
        pageLayout->addWidget(scroll, 1);

        auto *footer = new QHBoxLayout;
        auto *reload = new MotionButton(QStringLiteral("Сбросить изменения"));
        auto *save = new MotionButton(QStringLiteral("Сохранить настройки"));
        save->setObjectName(QStringLiteral("primary"));
        settingsStatus_ = new QLabel(QStringLiteral("Настройки ещё не загружены"));
        settingsStatus_->setObjectName(QStringLiteral("status"));
        footer->addWidget(settingsStatus_);
        footer->addStretch(1);
        footer->addWidget(reload);
        footer->addWidget(save);
        pageLayout->addLayout(footer);

        connect(gameDirBrowse, &QPushButton::clicked, this, [this] {
            const auto path = QFileDialog::getExistingDirectory(this, QStringLiteral("Папка игры"), settingsGameDir_->text().isEmpty() ? QDir::homePath() : settingsGameDir_->text());
            if (!path.isEmpty()) {
                settingsGameDir_->setText(path);
            }
        });
        connect(javaBrowse, &QPushButton::clicked, this, [this] {
            const auto path = QFileDialog::getOpenFileName(this, QStringLiteral("Java executable"), QDir::homePath());
            if (!path.isEmpty()) {
                settingsJavaPath_->setText(path);
            }
        });
        connect(showPathsButton, &QPushButton::clicked, this, [this] { showPaths(); });
        connect(reload, &QPushButton::clicked, this, [this] { loadSettings(); });
        connect(save, &QPushButton::clicked, this, [this] { saveSettingsInline(); });
    }

    void addAction(QHBoxLayout *layout, const QString &text, const std::function<void()> &handler, bool primary, const QString &icon = QString()) {
        auto *button = icon.isEmpty() ? static_cast<QPushButton *>(new MotionButton(text)) : static_cast<QPushButton *>(new MotionButton(launcherIcon(icon), text));
        if (primary) {
            button->setObjectName(QStringLiteral("primary"));
        }
        layout->addWidget(button);
        buttons_.append(button);
        connect(button, &QPushButton::clicked, this, handler);
    }

    QString backendRoot() const {
        QStringList candidates;
        candidates << QDir::currentPath() << QCoreApplication::applicationDirPath();
        QDir appDir(QCoreApplication::applicationDirPath());
        candidates << appDir.absoluteFilePath("../") << appDir.absoluteFilePath("../../");
        for (const auto &candidate : candidates) {
            const auto root = QDir(candidate).absolutePath();
            if (QFileInfo::exists(root + "/trial_launcher/__main__.py")) {
                return root;
            }
        }
        return {};
    }

    void runBackend(const QStringList &arguments, Operation operation, bool announce = true) {
        if (process_) {
            return;
        }
        operation_ = operation;
        output_.clear();
        progress_->setValue(0);
        if ((operation == Operation::Install || operation == Operation::Launch) && !progressDialog_) {
            progressDialog_ = new ProgressDialog(this);
            progressDialog_->setCancelHandler([this] {
                if (process_) {
                    process_->terminate();
                }
            });
            progressDialog_->show();
            progressDialog_->raise();
        }
        if (announce) {
            status_->setText(operationTitle(operation));
        }
        setBusy(true);
        process_ = new QProcess(this);
        auto environment = QProcessEnvironment::systemEnvironment();
        const auto root = backendRoot();
        if (!root.isEmpty()) {
            const auto current = environment.value(QStringLiteral("PYTHONPATH"));
            environment.insert(QStringLiteral("PYTHONPATH"), root + (current.isEmpty() ? QString() : QStringLiteral(":") + current));
            process_->setWorkingDirectory(root);
        }
        process_->setProcessEnvironment(environment);
        connect(process_, &QProcess::readyReadStandardOutput, this, [this] { readOutput(false); });
        connect(process_, &QProcess::readyReadStandardError, this, [this] { readOutput(true); });
        connect(process_, &QProcess::errorOccurred, this, [this](QProcess::ProcessError) {
            appendLog(QStringLiteral("[process error] %1").arg(process_->errorString()));
        });
        connect(process_, qOverload<int, QProcess::ExitStatus>(&QProcess::finished), this,
                [this](int code, QProcess::ExitStatus) { finishBackend(code); });
        QStringList pythonArguments{QStringLiteral("-u"), QStringLiteral("-m"), QStringLiteral("trial_launcher")};
        pythonArguments += arguments;
        process_->start(QStringLiteral("python3"), pythonArguments);
    }

    void readOutput(bool error) {
        if (!process_) {
            return;
        }
        const auto text = QString::fromLocal8Bit(error ? process_->readAllStandardError() : process_->readAllStandardOutput());
        output_ += text;
        const bool quietStructuredOutput = isStructuredOperation(operation_) && !error;
        const auto prefix = error ? QStringLiteral("[error] ") : QString();
        for (const auto &line : text.split('\n', Qt::SkipEmptyParts)) {
            const auto rendered = prefix + line.trimmed();
            if (rendered.trimmed().isEmpty()) {
                continue;
            }
            if (progressDialog_ && (operation_ == Operation::Install || operation_ == Operation::Launch)) {
                progressDialog_->appendLine(rendered);
                if (rendered.startsWith(QStringLiteral("[download] http"))) {
                    progressDialog_->setSource(rendered);
                }
            } else if (!quietStructuredOutput && !error) {
                appendLog(rendered);
            }
        }
        const auto match = QRegularExpression(QStringLiteral("(\\d{1,3})%")).match(text);
        if (match.hasMatch()) {
            const auto value = qBound(0, match.captured(1).toInt(), 100);
            progress_->setValue(value);
            if (progressDialog_) {
                progressDialog_->setProgress(value);
            }
        }
    }

    void finishBackend(int code) {
        if (!process_) {
            return;
        }
        readOutput(false);
        readOutput(true);
        const auto operation = operation_;
        process_->deleteLater();
        process_ = nullptr;
        if (progressDialog_) {
            progressDialog_->close();
            progressDialog_->deleteLater();
            progressDialog_ = nullptr;
        }
        setBusy(false);
        operation_ = Operation::None;
        if (code != 0) {
            status_->setText(QStringLiteral("Ошибка, код %1").arg(code));
            const auto detail = processErrorDetail();
            appendLog(QStringLiteral("Ошибка: %1").arg(detail));
            if (operation != Operation::Launch) {
                if (detail.contains(QStringLiteral("client_id"), Qt::CaseInsensitive)) {
                    const auto answer = QMessageBox::warning(this, QStringLiteral("Microsoft-вход"),
                                                              QStringLiteral("Нужен client ID зарегистрированного Microsoft OAuth-приложения. Откройте страницу настроек и укажите его один раз."),
                                                              QMessageBox::Open | QMessageBox::Cancel);
                    if (answer == QMessageBox::Open) {
                        setPage(3);
                    }
                } else {
                    QMessageBox::warning(this, QStringLiteral("Nivora Launcher"), detail);
                }
            }
            return;
        }
        status_->setText(QStringLiteral("Готово"));
        if (operation == Operation::Versions) {
            parseVersions();
            refreshAccounts();
        } else if (operation == Operation::LoaderVersions) {
            parseLoaderVersions();
            if (!settingsLoaded_) {
                loadSettings();
            }
        } else if (operation == Operation::Instances) {
            parseInstances();
        } else if (operation == Operation::Accounts) {
            parseAccounts();
            if (accountList_->count() == 0 && !offlineAutoCreated_) {
                offlineAutoCreated_ = true;
                runBackend({QStringLiteral("account"), QStringLiteral("offline"), QStringLiteral("Player")}, Operation::DebugAccount);
                return;
            }
            reloadLoaderVersions();
        } else if (operation == Operation::DebugAccount || operation == Operation::MicrosoftLogin) {
            refreshAccounts();
        } else if (operation == Operation::SettingsLoad) {
            populateSettings();
        } else if (operation == Operation::SettingsSave) {
            ++pendingSettingIndex_;
            saveNextSetting();
        } else if (operation == Operation::SkinsList || operation == Operation::SkinsImport) {
            parseSkins();
            if (operation == Operation::SkinsList && skinDialog_) {
                runBackend({QStringLiteral("skins"), QStringLiteral("profile"), skinAccountId_}, Operation::SkinsProfile);
            }
        } else if (operation == Operation::SkinsApply) {
            runBackend({QStringLiteral("skins"), QStringLiteral("list")}, Operation::SkinsList);
        } else if (operation == Operation::SkinsProfile) {
            parseCapes();
        } else if (operation == Operation::SkinsCapeApply) {
            runBackend({QStringLiteral("skins"), QStringLiteral("profile"), skinAccountId_}, Operation::SkinsProfile);
        }
    }

    void refreshVersions() {
        status_->setText(QStringLiteral("Загрузка списка версий..."));
        runBackend({QStringLiteral("versions"), QStringLiteral("--releases-only"), QStringLiteral("--limit"), QStringLiteral("200")}, Operation::Versions);
    }

    QString operationTitle(Operation operation) const {
        switch (operation) {
        case Operation::Versions:
            return QStringLiteral("Обновляю список версий...");
        case Operation::LoaderVersions:
            return QStringLiteral("Обновляю версии загрузчика...");
        case Operation::Instances:
            return QStringLiteral("Обновляю экземпляры...");
        case Operation::Accounts:
            return QStringLiteral("Обновляю аккаунты...");
        case Operation::Install:
            return QStringLiteral("Подготовка установки...");
        case Operation::Launch:
            return QStringLiteral("Запуск Minecraft...");
        case Operation::SettingsLoad:
            return QStringLiteral("Загружаю настройки...");
        case Operation::SettingsSave:
            return QStringLiteral("Сохраняю настройки...");
        case Operation::SkinsList:
        case Operation::SkinsImport:
        case Operation::SkinsApply:
        case Operation::SkinsProfile:
        case Operation::SkinsCapeApply:
            return QStringLiteral("Обновляю профиль скинов...");
        default:
            return QStringLiteral("Выполняю операцию...");
        }
    }

    bool isStructuredOperation(Operation operation) const {
        return operation == Operation::Versions || operation == Operation::LoaderVersions || operation == Operation::Instances ||
               operation == Operation::Accounts || operation == Operation::SettingsLoad || operation == Operation::SettingsSave ||
               operation == Operation::SkinsList || operation == Operation::SkinsImport || operation == Operation::SkinsApply ||
               operation == Operation::SkinsProfile || operation == Operation::SkinsCapeApply;
    }

    QString processErrorDetail() const {
        const auto lines = output_.split('\n', Qt::SkipEmptyParts);
        for (int index = lines.size() - 1; index >= 0; --index) {
            auto line = lines.at(index).trimmed();
            if (line.startsWith(QStringLiteral("Ошибка:"))) {
                line = line.mid(QStringLiteral("Ошибка:").size()).trimmed();
            }
            if (line.startsWith(QStringLiteral("[error]"))) {
                line = line.mid(QStringLiteral("[error]").size()).trimmed();
            }
            if (!line.isEmpty()) {
                return line.left(500);
            }
        }
        return QStringLiteral("Операция завершилась с ошибкой.");
    }

    void reloadLoaderVersions() {
        if (!loaderVersion_ || !loaderBox_ || !versionBox_) {
            return;
        }
        const auto loader = loaderBox_->currentText();
        const auto minecraftVersion = versionBox_->currentText();
        if (loader == QStringLiteral("vanilla") || minecraftVersion.isEmpty()) {
            loaderVersion_->clear();
            loaderVersion_->addItem(QStringLiteral("Для vanilla не требуется"), QString());
            loaderVersion_->setEnabled(false);
            if (!settingsLoaded_ && !process_) {
                loadSettings();
            }
            return;
        }
        loaderVersion_->clear();
        loaderVersion_->addItem(QStringLiteral("Загрузка версий..."), QString());
        loaderVersion_->setEnabled(false);
        status_->setText(QStringLiteral("Загрузка версий %1...").arg(loader));
        runBackend({QStringLiteral("loaders"), QStringLiteral("choices"), loader, minecraftVersion}, Operation::LoaderVersions);
    }

    void refreshInstances() {
        runBackend({QStringLiteral("instances"), QStringLiteral("list")}, Operation::Instances);
    }

    QString selectedInstance() const {
        if (instanceList_ && instanceList_->currentItem()) {
            return instanceList_->currentItem()->data(Qt::UserRole).toString();
        }
        return instanceName_->text().trimmed();
    }

    void duplicateInstance() {
        const auto source = selectedInstance();
        if (source.isEmpty()) {
            QMessageBox::information(this, QStringLiteral("Экземпляры"), QStringLiteral("Сначала выберите экземпляр."));
            return;
        }
        bool accepted = false;
        const auto target = QInputDialog::getText(this, QStringLiteral("Дублировать экземпляр"), QStringLiteral("Новое имя:"),
                                                  QLineEdit::Normal, source + QStringLiteral("-copy"), &accepted);
        if (accepted && !target.trimmed().isEmpty()) {
            runBackend({QStringLiteral("instances"), QStringLiteral("duplicate"), source, target.trimmed()}, Operation::Instances);
        }
    }

    void deleteInstance() {
        const auto name = selectedInstance();
        if (name.isEmpty()) {
            return;
        }
        if (QMessageBox::question(this, QStringLiteral("Удалить экземпляр"), QStringLiteral("Удалить «%1» без возможности восстановления?").arg(name)) == QMessageBox::Yes) {
            runBackend({QStringLiteral("instances"), QStringLiteral("delete"), name}, Operation::Instances);
        }
    }

    void exportInstance() {
        const auto name = selectedInstance();
        if (name.isEmpty()) {
            return;
        }
        const auto destination = QFileDialog::getSaveFileName(this, QStringLiteral("Экспорт экземпляра"), name + QStringLiteral(".zip"), QStringLiteral("ZIP archive (*.zip)"));
        if (!destination.isEmpty()) {
            runBackend({QStringLiteral("instances"), QStringLiteral("export"), name, destination}, Operation::Instances);
        }
    }

    void createInstanceShortcut() {
        const auto name = selectedInstance();
        if (name.isEmpty()) {
            return;
        }
        const auto directory = QFileDialog::getExistingDirectory(this, QStringLiteral("Папка для ярлыка"), QDir::homePath());
        if (!directory.isEmpty()) {
            runBackend({QStringLiteral("instances"), QStringLiteral("shortcut"), name, QStringLiteral("--directory"), directory}, Operation::Instances);
        }
    }

    void openInstanceSubfolder(const QString &subfolder) {
        auto *item = instanceList_ ? instanceList_->currentItem() : nullptr;
        if (!item) {
            QMessageBox::information(this, QStringLiteral("Экземпляры"), QStringLiteral("Сначала выберите экземпляр."));
            return;
        }
        const auto path = QDir(item->data(Qt::UserRole + 1).toString()).filePath(subfolder);
        QDir().mkpath(path);
        runBackend({QStringLiteral("open-folder"), path}, Operation::OpenFolder);
    }

    void stopBackend() {
        if (process_) {
            process_->terminate();
        }
    }

    void parseInstances() {
        const auto selected = instanceName_->text();
        instanceList_->clear();
        for (const auto &line : output_.split('\n', Qt::SkipEmptyParts)) {
            const auto fields = line.split('\t');
            if (fields.size() < 4) {
                continue;
            }
            auto *item = new QListWidgetItem(fields.at(0) + QStringLiteral("  ·  ") + fields.at(1) + QStringLiteral(" / ") + fields.at(2));
            item->setData(Qt::UserRole, fields.at(0));
            item->setData(Qt::UserRole + 1, fields.at(3));
            instanceList_->addItem(item);
        }
        for (int index = 0; index < instanceList_->count(); ++index) {
            if (instanceList_->item(index)->data(Qt::UserRole).toString() == selected) {
                instanceList_->setCurrentRow(index);
                return;
            }
        }
        if (instanceList_->count() > 0) {
            instanceList_->setCurrentRow(0);
        }
    }

    void parseVersions() {
        const auto selected = versionBox_->currentText();
        versionBox_->clear();
        const auto lines = output_.split('\n', Qt::SkipEmptyParts);
        const QRegularExpression versionPattern(QStringLiteral("^[0-9]+(?:\\.[0-9]+){1,2}(?:[-a-zA-Z0-9]+)?$"));
        for (const auto &line : lines) {
            const auto fields = line.simplified().split(' ', Qt::SkipEmptyParts);
            if (!fields.isEmpty() && versionPattern.match(fields.first()).hasMatch()) {
                versionBox_->addItem(fields.first());
            }
        }
        const auto oldIndex = versionBox_->findText(selected);
        if (oldIndex >= 0) {
            versionBox_->setCurrentIndex(oldIndex);
        }
    }

    void parseLoaderVersions() {
        const auto selected = loaderVersion_->currentData().toString();
        loaderVersion_->clear();
        loaderVersion_->addItem(QStringLiteral("Выберите версию загрузчика"), QString());
        const QRegularExpression pattern(QStringLiteral("^[A-Za-z0-9][-A-Za-z0-9._+/]*$"));
        for (const auto &line : output_.split('\n', Qt::SkipEmptyParts)) {
            const auto value = line.trimmed();
            if (pattern.match(value).hasMatch()) {
                loaderVersion_->addItem(value, value);
            }
        }
        loaderVersion_->setEnabled(loaderVersion_->count() > 1);
        const auto selectedIndex = loaderVersion_->findData(selected);
        loaderVersion_->setCurrentIndex(selectedIndex >= 0 ? selectedIndex : (loaderVersion_->count() > 1 ? 1 : 0));
    }

    void refreshAccounts() {
        runBackend({QStringLiteral("account"), QStringLiteral("list")}, Operation::Accounts);
    }

    void parseAccounts() {
        const auto selected = accountBox_->currentData().toString();
        accountBox_->clear();
        accountList_->clear();
        const QRegularExpression accountPattern(QStringLiteral("^([^:]+):\\s+(.+?)\\s+\\[(offline|microsoft)\\]"));
        for (const auto &line : output_.split('\n', Qt::SkipEmptyParts)) {
            const auto match = accountPattern.match(line.trimmed());
            if (!match.hasMatch()) {
                continue;
            }
            const auto label = match.captured(1) + QStringLiteral(": ") + match.captured(2);
            accountBox_->addItem(label, match.captured(3));
            auto *item = new QListWidgetItem(label + QStringLiteral("  ·  ") + match.captured(3));
            item->setData(Qt::UserRole, match.captured(1));
            item->setData(Qt::UserRole + 1, match.captured(2));
            accountList_->addItem(item);
        }
        if (accountBox_->count() == 0) {
            accountBox_->addItem(QStringLiteral("Аккаунт не выбран"), QStringLiteral("auto"));
        }
        const auto index = accountBox_->findData(selected);
        accountBox_->setCurrentIndex(index >= 0 ? index : 0);
        homeAccount_->setText(QStringLiteral("Аккаунт: ") + accountBox_->currentText());
    }

    void createDebugAccount() {
        bool accepted = false;
        const auto username = QInputDialog::getText(this, QStringLiteral("Debug-аккаунт"), QStringLiteral("Имя игрока:"),
                                                     QLineEdit::Normal, QStringLiteral("Tester"), &accepted);
        if (accepted && !username.trimmed().isEmpty()) {
            runBackend({QStringLiteral("account"), QStringLiteral("offline"), username.trimmed()}, Operation::DebugAccount);
        }
    }

    void createOfflineAccount() { createDebugAccount(); }

    void editSelectedAccount() {
        auto *item = accountList_->currentItem();
        if (!item) {
            QMessageBox::information(this, QStringLiteral("Аккаунты"), QStringLiteral("Сначала выберите аккаунт."));
            return;
        }
        bool accepted = false;
        const auto alias = QInputDialog::getText(this, QStringLiteral("Редактировать аккаунт"), QStringLiteral("Отображаемое имя:"),
                                                  QLineEdit::Normal, item->data(Qt::UserRole + 1).toString(), &accepted);
        if (accepted && !alias.trimmed().isEmpty()) {
            runBackend({QStringLiteral("account"), QStringLiteral("edit"), item->data(Qt::UserRole).toString(), alias.trimmed()}, Operation::Accounts);
        }
    }

    void openSkinManager() {
        auto *item = accountList_->currentItem();
        if (!item || item->text().contains(QStringLiteral("[offline]"))) {
            QMessageBox::information(this, QStringLiteral("Скины"), QStringLiteral("Сначала выберите Microsoft-аккаунт."));
            return;
        }
        QDialog dialog(this);
        skinDialog_ = &dialog;
        skinAccountId_ = item->data(Qt::UserRole).toString();
        dialog.setWindowTitle(QStringLiteral("Управление скинами"));
        dialog.resize(760, 540);
        auto *layout = new QHBoxLayout(&dialog);
        skinPreview_ = new SkinPreviewWidget;
        skinPreview_->setStyleSheet(QStringLiteral("background:#202938; border:1px solid #3b4961; border-radius:8px;"));
        layout->addWidget(skinPreview_);
        auto *controls = new QVBoxLayout;
        controls->addWidget(new QLabel(QStringLiteral("Локальная библиотека")));
        skinList_ = new QListWidget;
        controls->addWidget(skinList_, 1);
        skinModel_ = new QComboBox;
        skinModel_->addItem(QStringLiteral("Классическая модель"), QStringLiteral("classic"));
        skinModel_->addItem(QStringLiteral("Стройная модель"), QStringLiteral("slim"));
        controls->addWidget(skinModel_);
        skinCape_ = new QComboBox;
        skinCape_->addItem(QStringLiteral("Плащ не выбран"), QString());
        controls->addWidget(skinCape_);
        auto *import = new MotionButton(QStringLiteral("Импортировать PNG"));
        auto *apply = new MotionButton(QStringLiteral("Установить на Microsoft-аккаунт"));
        apply->setObjectName(QStringLiteral("primary"));
        auto *applyCape = new MotionButton(QStringLiteral("Установить выбранный плащ"));
        auto *clearCape = new MotionButton(QStringLiteral("Снять плащ"));
        controls->addWidget(import);
        controls->addWidget(apply);
        controls->addWidget(applyCape);
        controls->addWidget(clearCape);
        layout->addLayout(controls, 1);
        connect(skinList_, &QListWidget::currentItemChanged, &dialog, [this](QListWidgetItem *current) {
            if (!current || !skinPreview_) {
                return;
            }
            const auto image = QImage(current->data(Qt::UserRole + 1).toString());
            if (!image.isNull()) {
                skinPreview_->setSkinImage(image);
            }
        });
        connect(skinModel_, qOverload<int>(&QComboBox::currentIndexChanged), &dialog, [this] {
            if (skinPreview_) {
                skinPreview_->setSlim(skinModel_->currentData().toString() == QStringLiteral("slim"));
            }
        });
        connect(import, &QPushButton::clicked, &dialog, [this] {
            const auto file = QFileDialog::getOpenFileName(this, QStringLiteral("Выбрать скин"), QDir::homePath(), QStringLiteral("PNG skin (*.png)"));
            if (file.isEmpty()) {
                return;
            }
            const auto image = QImage(file);
            if (image.isNull() || (image.size() != QSize(64, 32) && image.size() != QSize(64, 64) && image.size() != QSize(128, 64) && image.size() != QSize(128, 128))) {
                QMessageBox::warning(this, QStringLiteral("Скины"), QStringLiteral("Нужен PNG-скин размером 64×32, 64×64, 128×64 или 128×128."));
                return;
            }
            runBackend({QStringLiteral("skins"), QStringLiteral("import"), file, QStringLiteral("--model"), skinModel_->currentData().toString()}, Operation::SkinsImport);
        });
        connect(apply, &QPushButton::clicked, &dialog, [this] {
            if (!skinList_->currentItem()) {
                QMessageBox::information(this, QStringLiteral("Скины"), QStringLiteral("Сначала выберите скин из библиотеки."));
                return;
            }
            runBackend({QStringLiteral("skins"), QStringLiteral("apply"), skinAccountId_, skinList_->currentItem()->data(Qt::UserRole).toString(),
                        QStringLiteral("--model"), skinModel_->currentData().toString()}, Operation::SkinsApply);
        });
        connect(applyCape, &QPushButton::clicked, &dialog, [this] {
            const auto capeId = skinCape_->currentData().toString();
            if (capeId.isEmpty()) {
                QMessageBox::information(this, QStringLiteral("Плащ"), QStringLiteral("Сначала выберите плащ."));
                return;
            }
            runBackend({QStringLiteral("skins"), QStringLiteral("cape-apply"), skinAccountId_, capeId}, Operation::SkinsCapeApply);
        });
        connect(clearCape, &QPushButton::clicked, &dialog, [this] {
            runBackend({QStringLiteral("skins"), QStringLiteral("cape-clear"), skinAccountId_}, Operation::SkinsCapeApply);
        });
        connect(&dialog, &QDialog::finished, this, [this] {
            skinDialog_ = nullptr;
            skinList_ = nullptr;
            skinPreview_ = nullptr;
            skinModel_ = nullptr;
            skinCape_ = nullptr;
        });
        runBackend({QStringLiteral("skins"), QStringLiteral("list")}, Operation::SkinsList);
        dialog.exec();
    }

    void parseSkins() {
        if (!skinList_) {
            return;
        }
        const auto selected = skinList_->currentItem() ? skinList_->currentItem()->data(Qt::UserRole).toString() : QString();
        skinList_->clear();
        for (const auto &line : output_.split('\n', Qt::SkipEmptyParts)) {
            const auto fields = line.split('\t');
            if (fields.size() < 4) {
                continue;
            }
            auto *item = new QListWidgetItem(fields.at(1) + QStringLiteral("  ·  ") + fields.at(2));
            item->setData(Qt::UserRole, fields.at(0));
            item->setData(Qt::UserRole + 1, fields.at(3));
            item->setData(Qt::UserRole + 2, fields.at(2));
            skinList_->addItem(item);
        }
        for (int index = 0; index < skinList_->count(); ++index) {
            if (skinList_->item(index)->data(Qt::UserRole).toString() == selected) {
                skinList_->setCurrentRow(index);
                return;
            }
        }
        if (skinList_->count() > 0) {
            skinList_->setCurrentRow(0);
        }
    }

    void parseCapes() {
        if (!skinCape_) {
            return;
        }
        skinCape_->clear();
        skinCape_->addItem(QStringLiteral("Плащ не выбран"), QString());
        QString active;
        for (const auto &line : output_.split('\n', Qt::SkipEmptyParts)) {
            const auto fields = line.split('\t');
            if (fields.size() < 4) {
                continue;
            }
            const auto state = fields.at(3).isEmpty() ? QString() : QStringLiteral("  ·  ") + fields.at(3);
            skinCape_->addItem(fields.at(1) + state, fields.at(0));
            if (fields.at(3) == QStringLiteral("active")) {
                active = fields.at(0);
            }
        }
        const auto index = skinCape_->findData(active);
        if (index >= 0) {
            skinCape_->setCurrentIndex(index);
        }
    }

    void showPaths(bool notify = true) {
        const auto home = QDir::homePath();
        const auto gameDir = settingsGameDir_ && !settingsGameDir_->text().trimmed().isEmpty()
                                 ? settingsGameDir_->text().trimmed()
                                 : home + QStringLiteral("/.local/share/trial-launcher/game");
        const auto text = QStringLiteral("Конфигурация: %1/.config/trial-launcher\nИгра и экземпляры: %2\nЛоги: %1/.local/share/trial-launcher/logs\nКэш загрузок: %1/.cache/trial-launcher").arg(home, gameDir);
        settingsPaths_->setText(text);
        if (notify) {
            QMessageBox::information(this, QStringLiteral("Каталоги Nivora Launcher"), text);
        }
    }

    void loginMicrosoft() {
        if (settingsLoaded_ && settingsClientId_->text().trimmed().isEmpty()) {
            const auto answer = QMessageBox::information(this, QStringLiteral("Microsoft-вход"),
                                                          QStringLiteral("Сначала укажите Microsoft OAuth client ID в настройках. Это требуется самой Microsoft и не создаётся автоматически."),
                                                          QMessageBox::Open | QMessageBox::Cancel);
            if (answer == QMessageBox::Open) {
                setPage(3);
            }
            return;
        }
        runBackend({QStringLiteral("login"), QStringLiteral("microsoft")}, Operation::MicrosoftLogin);
    }

    QStringList gameArguments(const QString &command) const {
        QStringList result{command, versionBox_->currentText(), QStringLiteral("--loader"), loaderBox_->currentText()};
        const auto loaderVersion = loaderVersion_->currentData().toString().trimmed();
        if (!loaderVersion.isEmpty()) {
            result << QStringLiteral("--loader-version") << loaderVersion;
        }
        if (!instanceName_->text().trimmed().isEmpty()) {
            result << QStringLiteral("--instance") << instanceName_->text().trimmed();
        }
        if (!memory_->text().trimmed().isEmpty()) {
            result << QStringLiteral("--memory") << memory_->text().trimmed();
        }
        if (noAssets_->isChecked()) {
            result << QStringLiteral("--no-assets");
        }
        if (command == QStringLiteral("launch")) {
            result << QStringLiteral("--account") << accountBox_->currentData().toString();
        }
        return result;
    }

    void install() {
        if (versionBox_->currentText().isEmpty()) {
            QMessageBox::information(this, QStringLiteral("Nivora Launcher"), QStringLiteral("Список версий ещё загружается."));
            return;
        }
        runBackend(gameArguments(QStringLiteral("install")), Operation::Install);
    }

    void launch() {
        if (versionBox_->currentText().isEmpty()) {
            QMessageBox::information(this, QStringLiteral("Nivora Launcher"), QStringLiteral("Список версий ещё загружается."));
            return;
        }
        runBackend(gameArguments(QStringLiteral("launch")), Operation::Launch);
    }

    void openFolder() {
        runBackend({QStringLiteral("open-folder")}, Operation::OpenFolder);
    }

    void loadSettings() {
        runBackend({QStringLiteral("config"), QStringLiteral("show"), QStringLiteral("--raw")}, Operation::SettingsLoad);
    }

    void populateSettings() {
        QJsonObject values;
        const auto start = output_.indexOf('{');
        const auto end = output_.lastIndexOf('}');
        if (start >= 0 && end > start) {
            values = QJsonDocument::fromJson(output_.mid(start, end - start + 1).toUtf8()).object();
        }
        settingsGameDir_->setText(values.value(QStringLiteral("game_dir")).toString());
        settingsClientId_->setText(values.value(QStringLiteral("client_id")).toString());
        settingsJavaPath_->setText(values.value(QStringLiteral("java_path")).toString());
        settingsMemory_->setText(values.value(QStringLiteral("memory")).toString());
        memory_->setText(settingsMemory_->text());
        settingsMaxDownloads_->setValue(values.value(QStringLiteral("max_downloads")).toInt(8));
        settingsLoaded_ = true;
        settingsStatus_->setText(QStringLiteral("Настройки загружены"));
        showPaths(false);
    }

    void saveSettingsInline() {
        pendingSettings_ = {
            {QStringLiteral("game_dir"), settingsGameDir_->text().trimmed()},
            {QStringLiteral("client_id"), settingsClientId_->text().trimmed()},
            {QStringLiteral("java_path"), settingsJavaPath_->text().trimmed()},
            {QStringLiteral("memory"), settingsMemory_->text().trimmed()},
            {QStringLiteral("max_downloads"), QString::number(settingsMaxDownloads_->value())},
        };
        pendingSettingIndex_ = 0;
        settingsStatus_->setText(QStringLiteral("Сохраняю..."));
        saveNextSetting();
    }

    void saveNextSetting() {
        if (pendingSettingIndex_ >= pendingSettings_.size()) {
            status_->setText(QStringLiteral("Настройки сохранены"));
            settingsStatus_->setText(QStringLiteral("Сохранено только что"));
            pendingSettings_.clear();
            return;
        }
        const auto &setting = pendingSettings_.at(pendingSettingIndex_);
        runBackend({QStringLiteral("config"), QStringLiteral("set"), setting.key, setting.value}, Operation::SettingsSave, false);
    }

    void setBusy(bool busy) {
        for (auto *button : buttons_) {
            button->setEnabled(!busy);
        }
        versionBox_->setEnabled(!busy);
        loaderBox_->setEnabled(!busy);
        loaderVersion_->setEnabled(!busy && loaderVersion_->count() > 1);
        accountBox_->setEnabled(!busy);
        settingsGameDir_->setEnabled(!busy);
        settingsClientId_->setEnabled(!busy);
        settingsJavaPath_->setEnabled(!busy);
        settingsMemory_->setEnabled(!busy);
        settingsMaxDownloads_->setEnabled(!busy);
    }

    void appendLog(const QString &text) {
        if (!text.trimmed().isEmpty()) {
            log_->appendPlainText(text.trimmed());
        }
    }
};

int main(int argc, char **argv) {
    QApplication application(argc, argv);
    LauncherWindow window;
    window.show();
    return application.exec();
}

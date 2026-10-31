"""Read-only Windows shell theme adapter."""
import sys

DEFAULT = ("#0d0e12", "#181a20", "#f4f5f7", "#8b919c", "#4f8cff", "#2a2d35")


def mix(a, b, fraction):
    return "#" + "".join(f"{round(int(a[i:i+2], 16)*(1-fraction)+int(b[i:i+2], 16)*fraction):02x}" for i in (1, 3, 5))


def luminance(colour):
    values = [int(colour[i:i+2], 16)/255 for i in (1, 3, 5)]
    return sum(weight*(v/12.92 if v <= 0.04045 else ((v+0.055)/1.055)**2.4)
               for weight, v in zip((0.2126, 0.7152, 0.0722), values))


def palette(background):
    # Keep the shell quiet; use the taskbar hue as a tint and focus accent.
    accent = mix("#4f8cff", mix(background, "#ffffff", 0.65), 0.35)
    return (mix("#0f1014", background, 0.06), mix("#1a1c22", background, 0.07),
            "#f4f4f5", "#8b919c", accent, "#2a2d35")


def windows_palette():
    if sys.platform != "win32":
        return DEFAULT
    import winreg
    def read(path, name, default=None):
        try:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, path) as key:
                return winreg.QueryValueEx(key, name)[0]
        except OSError:
            return default
    personalize = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"
    light = read(personalize, "SystemUsesLightTheme", 0)
    use_accent = read(personalize, "ColorPrevalence", 0)
    background = "#f3f3f3" if light else "#202020"
    if use_accent and not light:
        # Optional shell implementation detail (ABGR); DWM is the fallback.
        shade = read(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent", "StartColorMenu")
        if isinstance(shade, int):
            background = f"#{shade & 255:02x}{(shade >> 8) & 255:02x}{(shade >> 16) & 255:02x}"
        else:
            import ctypes
            from ctypes import wintypes
            colour, opaque = wintypes.DWORD(), wintypes.BOOL()
            try:
                if ctypes.windll.dwmapi.DwmGetColorizationColor(ctypes.byref(colour), ctypes.byref(opaque)) == 0:
                    background = f"#{colour.value & 0xffffff:06x}"
            except (OSError, AttributeError):
                pass
    return palette(background)

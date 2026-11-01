//! Draws the Windows tray menu with the app's palette while Windows handles its input.
use super::*;
use std::mem::size_of;
use tray_icon::menu::ContextMenu;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM},
    Graphics::Gdi::*,
    UI::{
        Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW},
        Controls::*,
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::*,
    },
};

const SUBCLASS: usize = 0x54504d;
const LABELS: [&str; 2] = ["Show TracePlotter", "Exit and stop monitoring"];

pub(super) struct Style {
    hwnd: HWND,
    state: Box<State>,
}
struct State {
    ctx: egui::Context,
    menu: HMENU,
    brush: HBRUSH,
    labels: [Vec<u16>; 2],
}

fn color(color: egui::Color32) -> u32 {
    u32::from(color.r()) | (u32::from(color.g()) << 8) | (u32::from(color.b()) << 16)
}

impl Style {
    pub fn new(hwnd: HWND, menu: &Menu, ctx: &egui::Context) -> Option<Self> {
        // Safety: these structures have their required sizes and live through each call.
        unsafe {
            let mut contrast = HIGHCONTRASTW {
                cbSize: size_of::<HIGHCONTRASTW>() as u32,
                ..Default::default()
            };
            if SystemParametersInfoW(
                SPI_GETHIGHCONTRAST,
                contrast.cbSize,
                (&raw mut contrast).cast(),
                0,
            ) != 0
                && contrast.dwFlags & HCF_HIGHCONTRASTON != 0
            {
                return None;
            }
            let brush = CreateSolidBrush(color(theme::palette(ctx).raised));
            if brush.is_null() {
                return None;
            }
            let mut style = Self {
                hwnd,
                state: Box::new(State {
                    ctx: ctx.clone(),
                    menu: menu.hpopupmenu() as HMENU,
                    brush,
                    labels: LABELS.map(|label| label.encode_utf16().collect()),
                }),
            };
            if SetWindowSubclass(hwnd, Some(draw), SUBCLASS, (&raw mut *style.state) as usize) == 0
            {
                return None;
            }
            for i in 0..LABELS.len() {
                let item = MENUITEMINFOW {
                    cbSize: size_of::<MENUITEMINFOW>() as u32,
                    fMask: MIIM_FTYPE | MIIM_DATA,
                    fType: MFT_OWNERDRAW,
                    dwItemData: i + 1,
                    ..Default::default()
                };
                SetMenuItemInfoW(style.state.menu, i as u32, 1, &item);
            }
            style.state.background();
            Some(style)
        }
    }
}

impl State {
    fn background(&mut self) {
        // Safety: the brush and menu remain owned by the tray for this call.
        unsafe {
            let brush = CreateSolidBrush(color(theme::palette(&self.ctx).raised));
            if brush.is_null() {
                return;
            }
            let info = MENUINFO {
                cbSize: size_of::<MENUINFO>() as u32,
                fMask: MIM_BACKGROUND,
                hbrBack: brush,
                ..Default::default()
            };
            if SetMenuInfo(self.menu, &info) != 0 {
                DeleteObject(self.brush);
                self.brush = brush;
            } else {
                DeleteObject(brush);
            }
        }
    }
}

unsafe extern "system" fn draw(
    hwnd: HWND,
    msg: u32,
    w: WPARAM,
    l: LPARAM,
    _: usize,
    data: usize,
) -> LRESULT {
    // Safety: the subclass is removed before its boxed state or tray window is dropped.
    unsafe {
        let state = &mut *(data as *mut State);
        if msg == WM_INITMENUPOPUP && w as HMENU == state.menu {
            state.background();
        }
        if msg == WM_MEASUREITEM && l != 0 {
            let item = &mut *(l as *mut MEASUREITEMSTRUCT);
            if item.CtlType == ODT_MENU && (1..=LABELS.len()).contains(&item.itemData) {
                let text = &state.labels[item.itemData - 1];
                let dc = GetDC(hwnd);
                let old = SelectObject(dc, GetStockObject(DEFAULT_GUI_FONT));
                let mut size = SIZE::default();
                GetTextExtentPoint32W(dc, text.as_ptr(), text.len() as i32, &mut size);
                SelectObject(dc, old);
                ReleaseDC(hwnd, dc);
                item.itemWidth = (size.cx + 32).max(0) as u32;
                item.itemHeight = (size.cy + 14).max(30) as u32;
                return 1;
            }
        }
        if msg == WM_DRAWITEM && l != 0 {
            let item = &*(l as *const DRAWITEMSTRUCT);
            if item.CtlType == ODT_MENU && (1..=LABELS.len()).contains(&item.itemData) {
                let palette = theme::palette(&state.ctx);
                let selected = item.itemState & ODS_SELECTED != 0;
                let brush = CreateSolidBrush(color(if selected {
                    palette.line
                } else {
                    palette.raised
                }));
                FillRect(item.hDC, &item.rcItem, brush);
                DeleteObject(brush);
                let saved = SaveDC(item.hDC);
                SelectObject(item.hDC, GetStockObject(DEFAULT_GUI_FONT));
                SetBkMode(item.hDC, TRANSPARENT as i32);
                SetTextColor(item.hDC, color(palette.text));
                let mut rect = RECT {
                    left: item.rcItem.left + 16,
                    ..item.rcItem
                };
                let text = &state.labels[item.itemData - 1];
                DrawTextW(
                    item.hDC,
                    text.as_ptr(),
                    text.len() as i32,
                    &mut rect,
                    DT_SINGLELINE | DT_VCENTER | DT_LEFT | DT_NOPREFIX,
                );
                RestoreDC(item.hDC, saved);
                return 1;
            }
        }
        DefSubclassProc(hwnd, msg, w, l)
    }
}

impl Drop for Style {
    fn drop(&mut self) {
        // Safety: Tray drops this guard before destroying its window and menu.
        unsafe {
            RemoveWindowSubclass(self.hwnd, Some(draw), SUBCLASS);
            let info = MENUINFO {
                cbSize: size_of::<MENUINFO>() as u32,
                fMask: MIM_BACKGROUND,
                ..Default::default()
            };
            SetMenuInfo(self.state.menu, &info);
            DeleteObject(self.state.brush);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_drawing_uses_the_current_theme_and_hover_colors() {
        let ctx = egui::Context::default();
        let menu = Menu::new();
        let mut state = State {
            ctx: ctx.clone(),
            menu: menu.hpopupmenu() as HMENU,
            brush: std::ptr::null_mut(),
            labels: LABELS.map(|label| label.encode_utf16().collect()),
        };
        // Safety: drawing uses an offscreen bitmap; every temporary GDI object is
        // restored or released before returning.
        unsafe {
            let screen = GetDC(std::ptr::null_mut());
            let dc = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, 300, 50);
            let old = SelectObject(dc, bitmap);
            ReleaseDC(std::ptr::null_mut(), screen);
            assert!(!dc.is_null() && !bitmap.is_null());
            let mut item = DRAWITEMSTRUCT {
                CtlType: ODT_MENU,
                itemData: 1,
                hDC: dc,
                rcItem: RECT {
                    left: 0,
                    top: 0,
                    right: 300,
                    bottom: 50,
                },
                ..Default::default()
            };
            for theme in [theme::Theme::Dark, theme::Theme::Glass] {
                theme::apply_theme(&ctx, theme);
                for selected in [false, true] {
                    item.itemState = if selected { ODS_SELECTED } else { 0 };
                    assert_eq!(
                        draw(
                            std::ptr::null_mut(),
                            WM_DRAWITEM,
                            0,
                            (&raw mut item) as LPARAM,
                            SUBCLASS,
                            (&raw mut state) as usize
                        ),
                        1
                    );
                    let palette = theme::palette(&ctx);
                    assert_eq!(
                        GetPixel(dc, 2, 2),
                        color(if selected {
                            palette.line
                        } else {
                            palette.raised
                        })
                    );
                }
            }
            SelectObject(dc, old);
            DeleteObject(bitmap);
            DeleteDC(dc);
        }
    }
}

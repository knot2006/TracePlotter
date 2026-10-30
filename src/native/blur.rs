//! The compositor's blur behind the window, so that what shows through Glass's translucent
//! panels is frosted rather than sharp. Windows 11 gives it as the acrylic backdrop. Wayland
//! compositors offer it through the standard background effect protocol, which Plasma 6.5
//! brought, or through Plasma's older one. Elsewhere the panels are translucent without blur,
//! unless the compositor blurs on its own.

/// How eframe should set up the GPU. On Windows only DirectX 12 drawing through
/// DirectComposition gives the window an alpha channel; a swap chain made straight from the
/// window, which is wgpu's default, is always opaque.
#[cfg(windows)]
pub(super) fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::wgpu;
    let mut setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    // WGPU_BACKEND still picks another backend, for a GPU whose DirectX 12 misbehaves.
    setup.instance_descriptor.backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::DX12);
    setup
        .instance_descriptor
        .backend_options
        .dx12
        .presentation_system = wgpu::Dx12SwapchainKind::DxgiFromVisual;
    eframe::egui_wgpu::WgpuConfiguration {
        wgpu_setup: setup.into(),
        ..Default::default()
    }
}
#[cfg(not(windows))]
pub(super) fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    Default::default()
}

#[cfg(windows)]
pub(super) fn request(cc: &eframe::CreationContext<'_>) -> Blur {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Graphics::Dwm::{
        DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
        DwmExtendFrameIntoClientArea, DwmSetWindowAttribute,
    };
    use windows_sys::Win32::UI::Controls::MARGINS;

    let Ok(RawWindowHandle::Win32(window)) = cc.window_handle().map(|h| h.as_raw()) else {
        return Blur::default();
    };
    let hwnd = window.hwnd.get() as windows_sys::Win32::Foundation::HWND;
    let set = |attribute: i32, value: i32| {
        // Safety: the window is the one eframe has just opened, and the value outlives the call.
        unsafe {
            DwmSetWindowAttribute(
                hwnd,
                attribute as u32,
                (&raw const value).cast(),
                size_of::<i32>() as u32,
            )
        }
    };
    // The backdrop shows only through the frame, so the frame is stretched over the whole
    // window. Failures leave an unblurred window and are not worth reporting: Windows 10 and
    // early Windows 11 have no acrylic backdrop.
    let everywhere = MARGINS {
        cxLeftWidth: -1,
        cxRightWidth: -1,
        cyTopHeight: -1,
        cyBottomHeight: -1,
    };
    // Safety: as above.
    unsafe { DwmExtendFrameIntoClientArea(hwnd, &everywhere) };
    // Acrylic follows the window's light or dark mode, and Glass is dark.
    set(DWMWA_USE_IMMERSIVE_DARK_MODE, 1);
    set(DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_TRANSIENTWINDOW);
    Blur::default()
}

#[cfg(target_os = "linux")]
pub(super) fn request(cc: &eframe::CreationContext<'_>) -> Blur {
    // Without the blur the window still works, so a compositor that lacks it is not an error.
    Blur {
        wayland: wayland::start(cc).ok().flatten(),
    }
}
#[cfg(not(any(windows, target_os = "linux")))]
pub(super) fn request(_cc: &eframe::CreationContext<'_>) -> Blur {
    Blur::default()
}

/// The blur behind the window, kept to the window's size from its first frame on.
#[derive(Default)]
pub(super) struct Blur {
    #[cfg(target_os = "linux")]
    wayland: Option<wayland::Effect>,
}
impl Blur {
    /// Called every frame; it speaks to the compositor only when the blur has to change.
    pub fn update(&mut self, _ctx: &eframe::egui::Context) {
        #[cfg(target_os = "linux")]
        if let Some(effect) = &mut self.wayland {
            effect.update(_ctx);
        }
    }
}

#[cfg(target_os = "linux")]
mod wayland {
    use eframe::egui;
    use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
    use wayland_client::backend::{Backend, ObjectId};
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::{
        wl_compositor::WlCompositor, wl_region::WlRegion, wl_registry, wl_surface::WlSurface,
    };
    use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
    use wayland_protocols::ext::background_effect::v1::client::{
        ext_background_effect_manager_v1::ExtBackgroundEffectManagerV1,
        ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1,
    };
    use wayland_protocols_plasma::blur::client::org_kde_kwin_blur::OrgKdeKwinBlur;
    use wayland_protocols_plasma::blur::client::org_kde_kwin_blur_manager::OrgKdeKwinBlurManager;

    pub(super) struct State;
    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
        fn event(
            _: &mut Self,
            _: &wl_registry::WlRegistry,
            _: wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }
    delegate_noop!(State: OrgKdeKwinBlurManager);
    delegate_noop!(State: OrgKdeKwinBlur);
    delegate_noop!(State: ignore ExtBackgroundEffectManagerV1);
    delegate_noop!(State: ExtBackgroundEffectSurfaceV1);
    delegate_noop!(State: WlCompositor);
    delegate_noop!(State: WlRegion);

    /// The standard protocol's blur, which has to be told the window's size.
    pub(super) struct Effect {
        connection: Connection,
        compositor: WlCompositor,
        surface: ExtBackgroundEffectSurfaceV1,
        /// The objects live on this queue, so it stays as long as they do.
        queue: EventQueue<State>,
        /// The size last blurred, in the surface's own units.
        blurred: (i32, i32),
    }
    impl Effect {
        pub(super) fn update(&mut self, ctx: &egui::Context) {
            // The blur stays on from the first frame, whatever the theme: behind opaque panels
            // it shows nothing, but KWin 6.8 keeps a blur that starts later stale, and
            // moving the window then shows old screen content through translucent panels.
            let size = ctx.content_rect().size();
            let wanted = (size.x.ceil() as i32, size.y.ceil() as i32);
            if wanted == self.blurred {
                return;
            }
            self.blurred = wanted;
            let region = self.compositor.create_region(&self.queue.handle(), ());
            region.add(0, 0, wanted.0, wanted.1);
            self.surface.set_blur_region(Some(&region));
            region.destroy();
            // The surface's next commit, which this frame makes, applies it.
            let _ = self.connection.flush();
        }
    }

    pub(super) fn start(
        cc: &eframe::CreationContext<'_>,
    ) -> Result<Option<Effect>, Box<dyn std::error::Error>> {
        let (RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(surface)) =
            (cc.display_handle()?.as_raw(), cc.window_handle()?.as_raw())
        else {
            return Ok(None);
        };
        // Safety: both belong to the window that eframe has just opened and keeps open.
        let connection = Connection::from_backend(unsafe {
            Backend::from_foreign_display(display.display.as_ptr().cast())
        });
        let surface = WlSurface::from_id(&connection, unsafe {
            ObjectId::from_ptr(WlSurface::interface(), surface.surface.as_ptr().cast())
        }?)?;
        let (globals, mut queue) = registry_queue_init::<State>(&connection)?;
        let handle = queue.handle();
        let effect = if let Ok(manager) =
            globals.bind::<ExtBackgroundEffectManagerV1, _, _>(&handle, 1..=1, ())
        {
            let compositor: WlCompositor = globals.bind(&handle, 1..=6, ())?;
            let surface = manager.get_background_effect(&surface, &handle, ());
            queue.roundtrip(&mut State)?;
            Some(Effect {
                connection,
                compositor,
                surface,
                queue,
                blurred: (0, 0),
            })
        } else {
            // Plasma's older protocol blurs the whole window when given no region, and
            // keeps doing so without being told the size.
            let manager: OrgKdeKwinBlurManager = globals.bind(&handle, 1..=1, ())?;
            manager.create(&surface, &handle, ()).commit();
            queue.roundtrip(&mut State)?;
            // The blur lasts as long as its object, so its queue stays for the app's life.
            std::mem::forget(queue);
            None
        };
        Ok(effect)
    }
}

//! Shared D3D11 device context for zero-copy VRAM sharing between
//! `Windows.Graphics.Capture`, `ID3D11VideoProcessor`, and Media Foundation.

use core_types::CaptureError;
use windows::core::Interface;
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice;

/// Shared hardware D3D11 device with multithread protection enabled.
#[derive(Clone)]
pub struct D3d11SharedContext {
    pub device: ID3D11Device,
    pub context: ID3D11DeviceContext,
    pub adapter_luid: u64,
}

// SAFETY: `D3d11SharedContext::new` enables `ID3D11Multithread::SetMultithreadProtected(true)`
// on the underlying D3D11 device, synchronizing concurrent D3D11 device/context calls across
// capture, video-processing, and Media Foundation hardware encoder threads.
unsafe impl Send for D3d11SharedContext {}
// SAFETY: Same multithread protection guarantee for shared immutable COM handles.
unsafe impl Sync for D3d11SharedContext {}

impl D3d11SharedContext {
    /// Creates a hardware D3D11 device with BGRA + Video support and multithread protection.
    pub fn new() -> Result<Self, CaptureError> {
        let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT;
        let feature_levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;

        unsafe {
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                Default::default(),
                flags,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(|err| CaptureError::InitFailed(format!("D3D11CreateDevice failed: {err}")))?;
        }

        let device = device.ok_or_else(|| {
            CaptureError::InitFailed("D3D11CreateDevice returned null device".to_string())
        })?;
        let context = context.ok_or_else(|| {
            CaptureError::InitFailed("D3D11CreateDevice returned null context".to_string())
        })?;

        // Enable multithread protection because the GPU device is shared across WGC,
        // ID3D11VideoProcessor, and Media Foundation hardware encoder threads.
        let multithread: ID3D11Multithread = device.cast().map_err(|err| {
            CaptureError::InitFailed(format!("Failed to query ID3D11Multithread: {err}"))
        })?;
        unsafe {
            let _ = multithread.SetMultithreadProtected(true);
        }

        let dxgi_device: IDXGIDevice = device.cast().map_err(|err| {
            CaptureError::InitFailed(format!("Failed to query IDXGIDevice: {err}"))
        })?;

        let adapter_luid = unsafe {
            let adapter = dxgi_device.GetAdapter().map_err(|err| {
                CaptureError::InitFailed(format!("IDXGIDevice::GetAdapter failed: {err}"))
            })?;
            let desc = adapter.GetDesc().map_err(|err| {
                CaptureError::InitFailed(format!("IDXGIAdapter::GetDesc failed: {err}"))
            })?;
            ((desc.AdapterLuid.HighPart as u64) << 32) | u64::from(desc.AdapterLuid.LowPart)
        };

        Ok(Self {
            device,
            context,
            adapter_luid,
        })
    }

    /// Creates a WinRT `IDirect3DDevice` wrapper bound to this D3D11 device on the caller thread.
    pub fn create_winrt_device(&self) -> Result<IDirect3DDevice, CaptureError> {
        let dxgi_device: IDXGIDevice = self.device.cast().map_err(|err| {
            CaptureError::InitFailed(format!("Failed to query IDXGIDevice: {err}"))
        })?;
        let inspectable = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device) }
            .map_err(|err| {
                CaptureError::InitFailed(format!(
                    "CreateDirect3D11DeviceFromDXGIDevice failed: {err}"
                ))
            })?;
        inspectable
            .cast()
            .map_err(|err| CaptureError::InitFailed(format!("Failed to cast IDirect3DDevice: {err}")))
    }

    /// Checks whether the underlying GPU device is still healthy (`GetDeviceRemovedReason`).
    pub fn check_device_healthy(&self) -> Result<(), CaptureError> {
        match unsafe { self.device.GetDeviceRemovedReason() } {
            Ok(()) => Ok(()),
            Err(err) => Err(CaptureError::DeviceRemoved {
                hresult: err.code().0 as u32,
            }),
        }
    }
}

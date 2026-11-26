//! Strict zero-copy GPU color-space conversion (`BGRA/RGBA16F -> NV12`) and scaling
//! using `ID3D11VideoProcessor` with a VRAM-resident NV12 output surface pool.

use crate::d3d_context::D3d11SharedContext;
use core_types::{CaptureError, GpuFrameDescriptor, PixelFormat};
use std::mem::ManuallyDrop;
use std::sync::Arc;
use windows::core::Interface;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoDevice,
    ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView,
    ID3D11VideoProcessorOutputView, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
    D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_VIDEO_COLOR, D3D11_VIDEO_COLOR_0, D3D11_VIDEO_COLOR_RGBA,
    D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_COLOR_SPACE,
    D3D11_VIDEO_PROCESSOR_CONTENT_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0, D3D11_VIDEO_PROCESSOR_STREAM,
    D3D11_VIDEO_USAGE_OPTIMAL_SPEED, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_FORMAT_R16G16B16A16_FLOAT,
    DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET};

const OUTPUT_POOL_SIZE: usize = 4;

/// VRAM-resident NV12 frame passed zero-copy from `core-capture` to `core-encode`.
#[derive(Clone)]
pub struct GpuTextureFrame {
    pub texture: ID3D11Texture2D,
    pub descriptor: GpuFrameDescriptor,
    pub source_epoch: u64,
    #[allow(dead_code)] // Held for RAII slot lifetime tracking via Arc::strong_count
    in_flight_guard: Arc<()>,
}

// SAFETY: `ID3D11Texture2D` belongs to a D3D11 device with `ID3D11Multithread` protection enabled,
// and `in_flight_guard` ensures the producer pool never overwrites a slot while `core-encode`
// still holds a `GpuTextureFrame`.
unsafe impl Send for GpuTextureFrame {}

struct OutputSlot {
    texture: ID3D11Texture2D,
    output_view: ID3D11VideoProcessorOutputView,
    in_flight: Arc<()>,
}

/// GPU-only `ID3D11VideoProcessor` converter and scaler (`BGRA/HDR -> NV12`).
pub struct GpuVideoProcessor {
    device: ID3D11Device,
    _context: ID3D11DeviceContext,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    adapter_luid: u64,
    enumerator: Option<ID3D11VideoProcessorEnumerator>,
    processor: Option<ID3D11VideoProcessor>,
    slots: Vec<OutputSlot>,
    slot_cursor: usize,
    configured_in: (u32, u32),
    configured_out: (u32, u32),
}

impl GpuVideoProcessor {
    /// Initializes D3D11 video processing interfaces on the shared GPU device.
    pub fn new(shared: &D3d11SharedContext) -> Result<Self, CaptureError> {
        let video_device: ID3D11VideoDevice = shared.device.cast().map_err(|err| {
            CaptureError::VideoProcessorFailed(format!("Query ID3D11VideoDevice failed: {err}"))
        })?;
        let video_context: ID3D11VideoContext = shared.context.cast().map_err(|err| {
            CaptureError::VideoProcessorFailed(format!("Query ID3D11VideoContext failed: {err}"))
        })?;

        Ok(Self {
            device: shared.device.clone(),
            _context: shared.context.clone(),
            video_device,
            video_context,
            adapter_luid: shared.adapter_luid,
            enumerator: None,
            processor: None,
            slots: Vec::with_capacity(OUTPUT_POOL_SIZE),
            slot_cursor: 0,
            configured_in: (0, 0),
            configured_out: (0, 0),
        })
    }

    /// Rebuilds the `ID3D11VideoProcessor` and NV12 VRAM surface pool if input/output geometry changed.
    pub fn ensure_configured(
        &mut self,
        in_width: u32,
        in_height: u32,
        out_width: u32,
        out_height: u32,
        fps: u32,
    ) -> Result<(), CaptureError> {
        let in_w = in_width.max(2);
        let in_h = in_height.max(2);
        // NV12 4:2:0 requires even dimensions.
        let out_w = (out_width.max(2)) & !1;
        let out_h = (out_height.max(2)) & !1;

        if self.processor.is_some()
            && self.configured_in == (in_w, in_h)
            && self.configured_out == (out_w, out_h)
            && !self.slots.is_empty()
        {
            return Ok(());
        }

        let fps_val = fps.max(1);
        let content_desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL {
                Numerator: fps_val,
                Denominator: 1,
            },
            InputWidth: in_w,
            InputHeight: in_h,
            OutputFrameRate: DXGI_RATIONAL {
                Numerator: fps_val,
                Denominator: 1,
            },
            OutputWidth: out_w,
            OutputHeight: out_h,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };

        let enumerator = unsafe { self.video_device.CreateVideoProcessorEnumerator(&content_desc) }
            .map_err(|err| map_d3d_err("CreateVideoProcessorEnumerator", err))?;

        let processor = unsafe { self.video_device.CreateVideoProcessor(&enumerator, 0) }
            .map_err(|err| map_d3d_err("CreateVideoProcessor", err))?;

        // Configure RGB Full Range (0..255) input -> YCbCr BT.709 Limited Range (16..235) output.
        let input_cs = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
            _bitfield: 2 << 4, // Nominal_Range = 2 (0..255), RGB_Range = 0 (Full)
        };
        let output_cs = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
            _bitfield: (1 << 2) | (1 << 4), // YCbCr_Matrix = 1 (BT.709), Nominal_Range = 1 (16..235)
        };

        let src_rect = RECT {
            left: 0,
            top: 0,
            right: in_w as i32,
            bottom: in_h as i32,
        };
        let dst_rect = fit_aspect_rect(in_w, in_h, out_w, out_h);
        let target_rect = RECT {
            left: 0,
            top: 0,
            right: out_w as i32,
            bottom: out_h as i32,
        };
        let black_bg = D3D11_VIDEO_COLOR {
            Anonymous: D3D11_VIDEO_COLOR_0 {
                RGBA: D3D11_VIDEO_COLOR_RGBA {
                    R: 0.0,
                    G: 0.0,
                    B: 0.0,
                    A: 1.0,
                },
            },
        };

        unsafe {
            self.video_context.VideoProcessorSetStreamFrameFormat(
                &processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );
            self.video_context
                .VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            self.video_context
                .VideoProcessorSetStreamColorSpace(&processor, 0, &input_cs);
            self.video_context
                .VideoProcessorSetOutputColorSpace(&processor, &output_cs);
            self.video_context
                .VideoProcessorSetOutputBackgroundColor(&processor, false, &black_bg);
            self.video_context
                .VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&src_rect));
            self.video_context
                .VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&dst_rect));
            self.video_context
                .VideoProcessorSetOutputTargetRect(&processor, true, Some(&target_rect));
        }

        // Rebuild NV12 output pool only when output dimensions changed.
        if self.configured_out != (out_w, out_h) || self.slots.is_empty() {
            self.slots.clear();
            let tex_desc = D3D11_TEXTURE2D_DESC {
                Width: out_w,
                Height: out_h,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_NV12,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };

            let vpov_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
                },
            };

            for _ in 0..OUTPUT_POOL_SIZE {
                let mut tex: Option<ID3D11Texture2D> = None;
                unsafe {
                    self.device
                        .CreateTexture2D(&tex_desc, None, Some(&mut tex))
                        .map_err(|err| map_d3d_err("CreateTexture2D(NV12)", err))?;
                }
                let texture = tex.ok_or_else(|| {
                    CaptureError::VideoProcessorFailed(
                        "CreateTexture2D(NV12) returned null".to_string(),
                    )
                })?;

                let mut vpov: Option<ID3D11VideoProcessorOutputView> = None;
                unsafe {
                    self.video_device
                        .CreateVideoProcessorOutputView(
                            &texture,
                            &enumerator,
                            &vpov_desc,
                            Some(&mut vpov),
                        )
                        .map_err(|err| map_d3d_err("CreateVideoProcessorOutputView", err))?;
                }
                let output_view = vpov.ok_or_else(|| {
                    CaptureError::VideoProcessorFailed(
                        "CreateVideoProcessorOutputView returned null".to_string(),
                    )
                })?;

                self.slots.push(OutputSlot {
                    texture,
                    output_view,
                    in_flight: Arc::new(()),
                });
            }
            self.slot_cursor = 0;
        }

        self.enumerator = Some(enumerator);
        self.processor = Some(processor);
        self.configured_in = (in_w, in_h);
        self.configured_out = (out_w, out_h);
        Ok(())
    }

    /// Converts and scales `input_tex` directly on the GPU into an NV12 VRAM texture.
    #[allow(clippy::too_many_arguments)]
    pub fn convert_to_nv12(
        &mut self,
        input_tex: &ID3D11Texture2D,
        out_width: u32,
        out_height: u32,
        fps: u32,
        pts_us: i64,
        frame_number: u64,
        source_epoch: u64,
    ) -> Result<GpuTextureFrame, CaptureError> {
        let mut in_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { input_tex.GetDesc(&mut in_desc) };

        self.ensure_configured(in_desc.Width, in_desc.Height, out_width, out_height, fps)?;

        let slot_idx = self.acquire_free_slot_index()?;
        let enumerator = self.enumerator.as_ref().ok_or_else(|| {
            CaptureError::VideoProcessorFailed("VideoProcessorEnumerator missing".to_string())
        })?;
        let processor = self.processor.as_ref().ok_or_else(|| {
            CaptureError::VideoProcessorFailed("VideoProcessor missing".to_string())
        })?;
        let slot = &self.slots[slot_idx];

        let vpiv_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: 0,
                },
            },
        };

        let mut vpiv: Option<ID3D11VideoProcessorInputView> = None;
        unsafe {
            self.video_device
                .CreateVideoProcessorInputView(
                    input_tex,
                    enumerator,
                    &vpiv_desc,
                    Some(&mut vpiv),
                )
                .map_err(|err| map_d3d_err("CreateVideoProcessorInputView", err))?;
        }
        let input_view = vpiv.ok_or_else(|| {
            CaptureError::VideoProcessorFailed(
                "CreateVideoProcessorInputView returned null".to_string(),
            )
        })?;

        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            OutputIndex: 0,
            InputFrameOrField: 0,
            PastFrames: 0,
            FutureFrames: 0,
            ppPastSurfaces: std::ptr::null_mut(),
            pInputSurface: ManuallyDrop::new(Some(input_view)),
            ppFutureSurfaces: std::ptr::null_mut(),
            ppPastSurfacesRight: std::ptr::null_mut(),
            pInputSurfaceRight: ManuallyDrop::new(None),
            ppFutureSurfacesRight: std::ptr::null_mut(),
        };

        let streams = [stream];
        let blt_res = unsafe {
            self.video_context
                .VideoProcessorBlt(processor, &slot.output_view, 0, &streams)
        };
        drop(ManuallyDrop::into_inner(unsafe {
            std::ptr::read(&streams[0].pInputSurface)
        }));

        blt_res.map_err(|err| map_d3d_err("VideoProcessorBlt", err))?;

        let is_hdr = in_desc.Format == DXGI_FORMAT_R16G16B16A16_FLOAT;
        Ok(GpuTextureFrame {
            texture: slot.texture.clone(),
            descriptor: GpuFrameDescriptor {
                pts_us,
                width: self.configured_out.0,
                height: self.configured_out.1,
                format: PixelFormat::Nv12,
                is_hdr,
                adapter_luid: self.adapter_luid,
                frame_number,
            },
            source_epoch,
            in_flight_guard: Arc::clone(&slot.in_flight),
        })
    }

    fn acquire_free_slot_index(&mut self) -> Result<usize, CaptureError> {
        let len = self.slots.len();
        for offset in 0..len {
            let idx = (self.slot_cursor + offset) % len;
            if Arc::strong_count(&self.slots[idx].in_flight) == 1 {
                self.slot_cursor = (idx + 1) % len;
                return Ok(idx);
            }
        }
        Err(CaptureError::VideoProcessorFailed(
            "All NV12 VRAM pool slots are still in flight".to_string(),
        ))
    }
}

/// Computes an aspect-preserving destination rectangle inside `out_w x out_h`.
fn fit_aspect_rect(in_w: u32, in_h: u32, out_w: u32, out_h: u32) -> RECT {
    if in_w == 0 || in_h == 0 || out_w == 0 || out_h == 0 {
        return RECT {
            left: 0,
            top: 0,
            right: out_w as i32,
            bottom: out_h as i32,
        };
    }

    let in_aspect = f64::from(in_w) / f64::from(in_h);
    let out_aspect = f64::from(out_w) / f64::from(out_h);

    let (dst_w, dst_h) = if (in_aspect - out_aspect).abs() < 0.01 {
        (out_w, out_h)
    } else if in_aspect > out_aspect {
        let h = ((f64::from(out_w) / in_aspect).round() as u32).clamp(2, out_h) & !1;
        (out_w, h)
    } else {
        let w = ((f64::from(out_h) * in_aspect).round() as u32).clamp(2, out_w) & !1;
        (w, out_h)
    };

    let left = ((out_w - dst_w) / 2) as i32;
    let top = ((out_h - dst_h) / 2) as i32;
    RECT {
        left,
        top,
        right: left + dst_w as i32,
        bottom: top + dst_h as i32,
    }
}

fn map_d3d_err(op: &str, err: windows::core::Error) -> CaptureError {
    let hr = err.code();
    if hr == DXGI_ERROR_DEVICE_REMOVED || hr == DXGI_ERROR_DEVICE_RESET {
        CaptureError::DeviceRemoved {
            hresult: hr.0 as u32,
        }
    } else {
        CaptureError::VideoProcessorFailed(format!("{op} failed: {err}"))
    }
}

/// Maps DXGI pixel formats to internal `PixelFormat`.
#[must_use]
pub fn map_dxgi_format(fmt: DXGI_FORMAT) -> PixelFormat {
    if fmt == DXGI_FORMAT_R16G16B16A16_FLOAT {
        PixelFormat::Rgba16FloatHdr
    } else if fmt == DXGI_FORMAT_B8G8R8A8_UNORM {
        PixelFormat::Bgra8Unorm
    } else {
        PixelFormat::Nv12
    }
}

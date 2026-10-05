//! Optional NVIDIA NVENC encoder backend (`encode-nvenc` feature, Windows).
//!
//! Uses a D3D11 ARGB texture + `register_resource_dx11` (the path exercised by
//! the upstream `nvenc` example). System-memory `InputBuffer` lock/unlock is
//! buggy in nvenc 0.1.0 (unlocks the data pointer instead of the buffer handle).

use rumpeg_util::{CodecParams, CodecSpecific, Error, Result, Timestamp, VideoFrame};

use nvenc::bitstream::BitStream;
use nvenc::encoder::{Encoder as NvEncoder, RegisteredResource};
use nvenc::session::{InitParams, Session};
use nvenc::sys::enums::{
    NVencBufferFormat, NVencPicStruct, NVencPicType, NVencTuningInfo,
};
use nvenc::sys::guids::{NV_ENC_CODEC_H264_GUID, NV_ENC_PRESET_P3_GUID};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BIND_SHADER_RESOURCE,
    D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION, D3D11_SUBRESOURCE_DATA, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::CreateDXGIFactory;

pub struct NvencBackend {
    _device: ID3D11Device,
    context: ID3D11DeviceContext,
    _texture: ID3D11Texture2D,
    encoder: NvEncoder,
    registered: RegisteredResource,
    bitstream: BitStream,
    argb: Vec<u8>,
    width: u32,
    height: u32,
    frame_idx: usize,
    gop: u32,
}

// NVENC/D3D11 objects are used from a single encoder thread; Encoder requires Send.
unsafe impl Send for NvencBackend {}

fn create_d3d11() -> Result<(ID3D11Device, ID3D11DeviceContext)> {
    let factory: windows::Win32::Graphics::Dxgi::IDXGIFactory =
        unsafe { CreateDXGIFactory() }
            .map_err(|e| Error::invalid_data(format!("DXGI factory: {e}")))?;
    let adapter = unsafe { factory.EnumAdapters(0) }
        .map_err(|e| Error::invalid_data(format!("DXGI adapter: {e}")))?;

    let mut device = None;
    let mut context = None;
    unsafe {
        D3D11CreateDevice(
            &adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE(std::ptr::null_mut()),
            D3D11_CREATE_DEVICE_FLAG(0),
            Some(&[D3D_FEATURE_LEVEL_11_0]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .map_err(|e| Error::invalid_data(format!("D3D11CreateDevice: {e}")))?;
    Ok((
        device.ok_or_else(|| Error::invalid_data("D3D11 device is null"))?,
        context.ok_or_else(|| Error::invalid_data("D3D11 context is null"))?,
    ))
}

fn create_argb_texture(device: &ID3D11Device, width: u32, height: u32) -> Result<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let zeros = vec![0u8; (width * height * 4) as usize];
    let data = D3D11_SUBRESOURCE_DATA {
        pSysMem: zeros.as_ptr() as *const _,
        SysMemPitch: width * 4,
        SysMemSlicePitch: 0,
    };
    let mut texture = None;
    unsafe { device.CreateTexture2D(&desc, Some(&data), Some(&mut texture)) }
        .map_err(|e| Error::invalid_data(format!("CreateTexture2D: {e}")))?;
    texture.ok_or_else(|| Error::invalid_data("D3D11 texture is null"))
}

fn yuv420_to_argb(video: &VideoFrame, dst: &mut [u8], width: usize, height: usize) -> Result<()> {
    if video.format != rumpeg_util::PixelFormat::Yuv420p {
        return Err(Error::invalid_data("NVENC expects yuv420p"));
    }
    let y = video
        .plane(0)
        .ok_or_else(|| Error::invalid_data("missing Y"))?;
    let u = video
        .plane(1)
        .ok_or_else(|| Error::invalid_data("missing U"))?;
    let v = video
        .plane(2)
        .ok_or_else(|| Error::invalid_data("missing V"))?;
    let src_w = video.width as usize;
    let src_h = video.height as usize;
    for row in 0..height.min(src_h) {
        for col in 0..width.min(src_w) {
            let yi = y[row * src_w + col] as i32;
            let ui = u[(row / 2) * (src_w / 2) + (col / 2)] as i32;
            let vi = v[(row / 2) * (src_w / 2) + (col / 2)] as i32;
            let c = yi - 16;
            let d = ui - 128;
            let e = vi - 128;
            let r = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
            let g = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
            let b = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;
            let i = (row * width + col) * 4;
            dst[i] = r;
            dst[i + 1] = g;
            dst[i + 2] = b;
            dst[i + 3] = 255;
        }
    }
    Ok(())
}

impl NvencBackend {
    pub fn open(params: &CodecParams) -> Result<(Self, Vec<u8>)> {
        let video = match &params.specific {
            CodecSpecific::Video(v) if v.width > 0 && v.height > 0 => v.clone(),
            _ => {
                return Err(Error::invalid_data(
                    "H.264 encoder requires video width/height",
                ));
            }
        };
        let width = video.width.max(2);
        let height = video.height.max(2);
        let fps = video.frame_rate.as_f64();
        let fps_num = if fps > 0.0 {
            fps.round().max(1.0) as u32
        } else {
            25
        };
        let gop = if params.gop_size > 0 {
            params.gop_size
        } else {
            30
        };

        let (device, context) = create_d3d11()?;
        let texture = create_argb_texture(&device, width, height)?;

        let session: Session<nvenc::session::NeedsConfig> = Session::open_dx(&device)
            .map_err(|e| Error::invalid_data(format!("NVENC session: {e:?}")))?;

        let codecs = session
            .get_encode_codecs()
            .map_err(|e| Error::invalid_data(format!("NVENC codecs: {e:?}")))?;
        if !codecs.iter().any(|g| g == &NV_ENC_CODEC_H264_GUID) {
            return Err(Error::unsupported(
                "NVENC H.264 not available on this GPU",
            ));
        }

        let (session, mut config) = session
            .get_encode_preset_config_ex(
                NV_ENC_CODEC_H264_GUID,
                NV_ENC_PRESET_P3_GUID,
                NVencTuningInfo::HighQuality,
            )
            .map_err(|e| Error::invalid_data(format!("NVENC preset: {e:?}")))?;

        config.preset_cfg.gop_len = gop;
        config.preset_cfg.frame_interval_p = 1;
        if params.bit_rate > 0 {
            config.preset_cfg.rc_params.rate_control_mode =
                nvenc::sys::enums::NVencParamsRcMode::VBR;
            config.preset_cfg.rc_params.average_bit_rate = params.bit_rate as u32;
        } else if params.quality >= 0 {
            let qp = params.quality.clamp(0, 51) as u32;
            let pixels = (width as u64) * (height as u64) * fps_num as u64;
            let scale = ((51 - qp) as u64 + 8) * 2;
            let br = (pixels * scale / 1000).clamp(100_000, 20_000_000) as u32;
            config.preset_cfg.rc_params.rate_control_mode =
                nvenc::sys::enums::NVencParamsRcMode::VBR;
            config.preset_cfg.rc_params.average_bit_rate = br;
        }

        let init_params = InitParams {
            encode_guid: NV_ENC_CODEC_H264_GUID,
            preset_guid: NV_ENC_PRESET_P3_GUID,
            aspect_ratio: [width, height],
            encode_config: &mut config.preset_cfg,
            tuning_info: NVencTuningInfo::HighQuality,
            buffer_format: NVencBufferFormat::ARGB,
            frame_rate: [fps_num, 1],
            resolution: [width, height],
            enable_ptd: true,
            max_encoder_resolution: [0, 0],
        };

        let encoder = session
            .init_encoder(init_params)
            .map_err(|e| Error::invalid_data(format!("NVENC init: {e:?}")))?;

        let registered = encoder
            .register_resource_dx11(&texture, NVencBufferFormat::ARGB, width * 4)
            .map_err(|e| Error::invalid_data(format!("NVENC register: {e:?}")))?;
        let bitstream = encoder
            .create_bitstream_buffer()
            .map_err(|e| Error::invalid_data(format!("NVENC bitstream: {e:?}")))?;

        let mut backend = Self {
            _device: device,
            context,
            _texture: texture,
            encoder,
            registered,
            bitstream,
            argb: vec![0u8; (width * height * 4) as usize],
            width,
            height,
            frame_idx: 0,
            gop,
        };

        let black = VideoFrame::alloc(rumpeg_util::PixelFormat::Yuv420p, width, height);
        let (headers, _, _) = backend
            .encode_raw(&black, Timestamp::NONE)?
            .ok_or_else(|| Error::invalid_data("NVENC produced empty probe frame"))?;
        let avcc = super::avcc_from_annexb(&headers)?;
        backend.frame_idx = 0;

        Ok((backend, avcc))
    }

    fn encode_raw(
        &mut self,
        video: &VideoFrame,
        pts: Timestamp,
    ) -> Result<Option<(Vec<u8>, Timestamp, bool)>> {
        yuv420_to_argb(video, &mut self.argb, self.width as usize, self.height as usize)?;
        let pitch = self.width * 4;
        unsafe {
            self.context.UpdateSubresource(
                &self._texture,
                0,
                None,
                self.argb.as_ptr() as *const _,
                pitch,
                0,
            );
        }

        let force_idr =
            self.frame_idx == 0 || (self.gop > 0 && (self.frame_idx as u32) % self.gop == 0);
        let pic_type = if force_idr {
            NVencPicType::IDR
        } else {
            NVencPicType::P
        };
        self.encoder
            .encode_picture(
                &self.registered,
                &self.bitstream,
                self.frame_idx,
                self.frame_idx as u64,
                NVencBufferFormat::ARGB,
                NVencPicStruct::Frame,
                pic_type,
                None,
            )
            .map_err(|e| Error::invalid_data(format!("NVENC encode: {e:?}")))?;
        self.frame_idx += 1;

        let lock = self
            .bitstream
            .try_lock(true)
            .map_err(|e| Error::invalid_data(format!("NVENC lock bitstream: {e:?}")))?;
        let bytes = lock.as_slice().to_vec();
        drop(lock);
        if bytes.is_empty() {
            return Ok(None);
        }
        let key = force_idr || super::annexb_is_keyframe(&bytes);
        Ok(Some((bytes, pts, key)))
    }

    pub fn encode(&mut self, video: &VideoFrame) -> Result<Option<(Vec<u8>, Timestamp, bool)>> {
        self.encode_raw(video, video.pts)
    }

    pub fn flush(&mut self) -> Result<Vec<(Vec<u8>, Timestamp, bool)>> {
        let _ = self.encoder.end_encode();
        Ok(Vec::new())
    }
}

module waywallen.image.av;
import wavsen.ffi.ffmpeg;
import waywallen.bridge;

using namespace rstd::prelude;
using namespace rstd::literals;
using namespace wavsen::ffi::ffmpeg;
using rstd::int32_t;
using rstd::size_t;
using rstd::uint32_t;
using rstd::uint8_t;

namespace ww_image
{

namespace
{

bool fail(DecodeError* err, ref<str> message) {
    if (err) err->message = String::make(message);
    return false;
}

String av_err_str(int rc) {
    char buf[AV_ERROR_MAX_STRING_SIZE] = {};
    av_strerror(rc, buf, sizeof(buf));
    return String::make(rstd::ffi::CStr::from_ptr(buf).to_str().unwrap_or(""_str));
}

} // namespace

struct ImageSession::Impl {
    String    path;
    FmtCtxPtr fmt;
    int       video_index;
    uint32_t  width;
    uint32_t  height;
};

ImageSession::ImageSession(Box<Impl> impl): impl_(rstd::move(impl)) {}
ImageSession::ImageSession(ImageSession&&) noexcept                    = default;
auto ImageSession::operator=(ImageSession&&) noexcept -> ImageSession& = default;
ImageSession::~ImageSession()                                          = default;
uint32_t ImageSession::width() const { return impl_->width; }
uint32_t ImageSession::height() const { return impl_->height; }

auto ImageSession::open(ref<str> path, DecodeError* err) -> Option<ImageSession> {
    auto terminated = rstd::ffi::CString::make(Vec<u8>::from(path.as_bytes()));
    if (terminated.is_err()) {
        fail(err, "image path contains a null byte"_str);
        return None();
    }
    AVFormatContext* raw_fmt = nullptr;
    if (int rc = avformat_open_input(&raw_fmt, terminated->as_ptr(), nullptr, nullptr); rc < 0) {
        fail(err, rstd::format("avformat_open_input: {}", av_err_str(rc)));
        return None();
    }
    FmtCtxPtr fmt(raw_fmt);

    if (int rc = avformat_find_stream_info(fmt.get(), nullptr); rc < 0) {
        fail(err, rstd::format("avformat_find_stream_info: {}", av_err_str(rc)));
        return None();
    }

    int video_idx = -1;
    for (unsigned i = 0; i < fmt->nb_streams; ++i) {
        if (fmt->streams[i]->codecpar->codec_type == AVMEDIA_TYPE_VIDEO) {
            video_idx = static_cast<int>(i);
            break;
        }
    }
    if (video_idx < 0) {
        fail(err, "no video/image stream in file"_str);
        return None();
    }

    const auto* par = fmt->streams[video_idx]->codecpar;
    if (par->width <= 0 || par->height <= 0) {
        fail(err, "invalid image dimensions"_str);
        return None();
    }
    auto impl         = Box<Impl>::make();
    impl->path        = String::make(path);
    impl->video_index = video_idx;
    impl->width       = static_cast<uint32_t>(par->width);
    impl->height      = static_cast<uint32_t>(par->height);
    impl->fmt         = rstd::move(fmt);
    return Some(ImageSession(rstd::move(impl)));
}

RgbaBuf ImageSession::decode(uint32_t target_w, uint32_t target_h, DecodeError* err,
                             bool allow_upscale) {
    RgbaBuf out;
    if (! target_w || ! target_h ||
        (! allow_upscale && (target_w > width() || target_h > height()))) {
        fail(err, "invalid image target extent"_str);
        return out;
    }
    if (! impl_->fmt) {
        auto reopened = open(impl_->path, err);
        if (! reopened) return out;
        if (reopened->width() != width() || reopened->height() != height()) {
            fail(err, "image dimensions changed"_str);
            return out;
        }
        impl_ = rstd::move(reopened->impl_);
    }
    auto               fmt       = rstd::move(impl_->fmt);
    int                video_idx = impl_->video_index;
    AVStream*          st        = fmt->streams[video_idx];
    AVCodecParameters* par       = st->codecpar;
    const AVCodec*     dec       = avcodec_find_decoder(par->codec_id);
    if (! dec) {
        fail(err, rstd::format("no decoder for codec {}", avcodec_get_name(par->codec_id)));
        return out;
    }

    CodecCtxPtr cctx(avcodec_alloc_context3(dec));
    if (! cctx) {
        fail(err, "avcodec_alloc_context3 failed"_str);
        return out;
    }
    if (int rc = avcodec_parameters_to_context(cctx.get(), par); rc < 0) {
        fail(err, rstd::format("avcodec_parameters_to_context: {}", av_err_str(rc)));
        return out;
    }
    if (int rc = avcodec_open2(cctx.get(), dec, nullptr); rc < 0) {
        fail(err, rstd::format("avcodec_open2: {}", av_err_str(rc)));
        return out;
    }

    PacketPtr pkt(av_packet_alloc());
    FramePtr  src_frame(av_frame_alloc());
    if (! pkt || ! src_frame) {
        fail(err, "av_packet_alloc / av_frame_alloc failed"_str);
        return out;
    }

    // Static images use only the first decoded frame.
    bool got_frame = false;
    while (! got_frame) {
        int rc = av_read_frame(fmt.get(), pkt.get());
        if (rc == AVERROR_EOF) {
            // Flush the decoder.
            avcodec_send_packet(cctx.get(), nullptr);
        } else if (rc < 0) {
            fail(err, rstd::format("av_read_frame: {}", av_err_str(rc)));
            return out;
        } else if (pkt->stream_index != video_idx) {
            av_packet_unref(pkt.get());
            continue;
        } else {
            rc = avcodec_send_packet(cctx.get(), pkt.get());
            av_packet_unref(pkt.get());
            if (rc < 0 && rc != AVERROR(EAGAIN)) {
                fail(err, rstd::format("avcodec_send_packet: {}", av_err_str(rc)));
                return out;
            }
        }

        while (true) {
            rc = avcodec_receive_frame(cctx.get(), src_frame.get());
            if (rc == AVERROR(EAGAIN)) break;
            if (rc == AVERROR_EOF) {
                fail(err, "decoder flushed without producing a frame"_str);
                return out;
            }
            if (rc < 0) {
                fail(err, rstd::format("avcodec_receive_frame: {}", av_err_str(rc)));
                return out;
            }
            got_frame = true;
            break;
        }
    }

    const auto src_fmt = static_cast<AVPixelFormat>(src_frame->format);
    const int  src_w   = src_frame->width;
    const int  src_h   = src_frame->height;
    if (src_w <= 0 || src_h <= 0 || src_fmt == AV_PIX_FMT_NONE) {
        fail(err, "decoded frame has invalid dimensions/format"_str);
        return out;
    }

    SwsPtr sws(sws_getContext(src_w,
                              src_h,
                              src_fmt,
                              static_cast<int>(target_w),
                              static_cast<int>(target_h),
                              AV_PIX_FMT_RGBA,
                              SWS_BICUBIC,
                              nullptr,
                              nullptr,
                              nullptr));
    if (! sws) {
        fail(err, rstd::format("sws_getContext failed (src={})", av_get_pix_fmt_name(src_fmt)));
        return out;
    }

    const uint32_t stride = target_w * 4;
    const size_t   bytes  = static_cast<size_t>(stride) * target_h;
    // Vec<u8> uses bulk zero-fill, including FFmpeg's required padding.
    out.data.resize(usize(bytes + AV_INPUT_BUFFER_PADDING_SIZE), u8());

    uint8_t* dst_planes[4] = {
        reinterpret_cast<uint8_t*>(out.data.data()), nullptr, nullptr, nullptr
    };
    int dst_strides[4] = { static_cast<int>(stride), 0, 0, 0 };

    int scaled = sws_scale(
        sws.get(), src_frame->data, src_frame->linesize, 0, src_h, dst_planes, dst_strides);
    if (scaled <= 0) {
        fail(err, "sws_scale produced no rows"_str);
        out.data.clear();
        return out;
    }

    out.width = target_w;
    out.data.truncate(usize(bytes));
    out.height = target_h;
    out.stride = stride;
    return out;
}

RgbaBuf decode_to_rgba(ref<str> path, int32_t resolution, DecodeError* err) {
    auto session = ImageSession::open(path, err);
    if (! session) return {};
    uint32_t width = session->width(), height = session->height();
    ww_resolution_apply_cap(resolution, WW_RESOLUTION_CAP_DEFAULT, &width, &height);
    return session->decode(width, height, err);
}

} // namespace ww_image

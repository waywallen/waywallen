module;
#include "control.qpb.h"

export module waywallen:proto;

namespace proto = waywallen::control::v1;

export namespace waywallen::control::v1
{
using proto::StatusGadget::Status;

using proto::Request;
using proto::Response;
using proto::Empty;

using proto::HealthRequest;
using proto::HealthResponse;

using proto::RendererSpawnRequest;
using proto::RendererSpawnResponse;
using proto::RendererListRequest;
using proto::RendererListResponse;
using proto::RendererPlayRequest;
using proto::RendererPauseRequest;
using proto::RendererMouseRequest;
using proto::RendererFpsRequest;
using proto::RendererKillRequest;

using proto::RendererPluginListRequest;
using proto::RendererPluginListResponse;
using proto::RendererPluginInfo;

using proto::WallpaperEntry;
using proto::WallpaperListRequest;
using proto::WallpaperListResponse;
using proto::WallpaperScanRequest;
using proto::WallpaperScanResponse;
using proto::WallpaperApplyRequest;
using proto::WallpaperApplyResponse;

using proto::SourceListRequest;
using proto::SourceListResponse;
using proto::SourcePluginInfo;
} // namespace waywallen::control::v1

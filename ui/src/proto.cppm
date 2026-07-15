module;
#include "control.qpb.h"

export module waywallen:proto;

namespace proto = waywallen::control::v1;

export namespace waywallen::control::v1
{
using proto::StatusGadget::Status;

using proto::DisplayChanged;
using proto::DisplayRemoved;
using proto::DisplaySnapshot;
using proto::Empty;
using proto::Event;
using proto::Request;
using proto::Response;
using proto::ServerFrame;

using proto::HealthRequest;
using proto::HealthResponse;

using proto::RendererFpsRequest;
using proto::RendererInstance;
using proto::RendererKillRequest;
using proto::RendererListRequest;
using proto::RendererListResponse;
using proto::RendererMouseRequest;
using proto::RendererPauseRequest;
using proto::RendererPlayRequest;
using proto::RendererSpawnRequest;
using proto::RendererSpawnResponse;

using proto::RendererPluginInfo;
using proto::RendererPluginListRequest;
using proto::RendererPluginListResponse;
using proto::SettingSchema;

using proto::WallpaperApplyRequest;
using proto::WallpaperApplyResponse;
using proto::WallpaperApplyViaPortalRequest;
using proto::WallpaperApplyViaPortalResponse;
using proto::WallpaperEntry;
using proto::WallpaperGetRequest;
using proto::WallpaperGetResponse;
using proto::WallpaperLayoutSetRequest;
using proto::WallpaperLayoutSetResponse;
using proto::WallpaperListRequest;
using proto::WallpaperListResponse;
using proto::WallpaperPropertySetRequest;
using proto::WallpaperPropertySetResponse;
using proto::WallpaperRemoveRequest;
using proto::WallpaperRemoveResponse;
using proto::WallpaperScanRequest;
using proto::WallpaperScanResponse;
using proto::WallpaperSyncFinished;

using proto::DisplayBackendStatus;
using proto::StatusSync;
using proto::TaskProgress;
using proto::DaemonPhaseGadget::DaemonPhase;

using proto::SourceListRequest;
using proto::SourceListResponse;
using proto::SourcePluginInfo;

using proto::DisplayInfo;
using proto::DisplayLayoutSetRequest;
using proto::DisplayLayoutSetResponse;
using proto::DisplayLinkInfo;
using proto::DisplayListRequest;
using proto::DisplayListResponse;
using proto::DisplayRenameRequest;
using proto::DisplayRenameResponse;
using proto::DisplayLockWallpaperSetRequest;
using proto::DisplayLockWallpaperSetResponse;
using proto::LayoutOverride;

using proto::RemoteAvailabilityRequest;
using proto::RemoteAvailabilityResponse;
using proto::RemoteDetailsRequest;
using proto::RemoteDetailsResponse;
using proto::RemoteDownloadProgress;
using proto::RemoteDownloadRequest;
using proto::RemoteDownloadResponse;
using proto::RemoteItem;
using proto::RemoteSearchRequest;
using proto::RemoteSearchResponse;
using proto::RemoteSortOption;
using proto::RemoteSourceInfo;
using proto::RemoteUninstallRequest;
using proto::RemoteUninstallResponse;
using proto::RemoteDownloadStateGadget::RemoteDownloadState;

using proto::GpuInfo;
using proto::GpuListRequest;
using proto::GpuListResponse;

using proto::PluginDeleteRequest;
using proto::PluginDeleteResponse;
using proto::PluginInfo;
using proto::PluginInspectRequest;
using proto::PluginInspectResponse;
using proto::PluginInstallRequest;
using proto::PluginInstallResponse;
using proto::PluginListRequest;
using proto::PluginListResponse;
using proto::PluginUpdateCheckRequest;
using proto::PluginUpdateCheckResponse;
using proto::PluginUpdateInfo;
using proto::PluginUpdateInstallRequest;
using proto::PluginUpdateInstallResponse;
using proto::PluginUpdateStateGadget::PluginUpdateState;

using proto::ContentRatingListRequest;
using proto::ContentRatingListResponse;
using proto::TagListRequest;
using proto::TagListResponse;

using proto::LibraryAddRequest;
using proto::LibraryAutoDetectRequest;
using proto::LibraryAutoDetectResponse;
using proto::LibraryChanged;
using proto::LibraryInstance;
using proto::LibraryListRequest;
using proto::LibraryListResponse;
using proto::LibraryRemoved;
using proto::LibraryRemoveRequest;
using proto::LibrarySnapshot;

using proto::AutoReplayPolicy;
using proto::AutostartGetRequest;
using proto::AutostartGetResponse;
using proto::AutostartSetRequest;
using proto::AutostartSetResponse;
using proto::GlobalSettings;
using proto::LayoutPrefs;
using proto::PluginSettings;
using proto::SettingsChanged;
using proto::SettingsGetRequest;
using proto::SettingsGetResponse;
using proto::SettingsSetRequest;
using proto::AlignGadget::Align;
using proto::AutoActionGadget::AutoAction;
using proto::FillModeGadget::FillMode;
using proto::LayoutSourceGadget::LayoutSource;
using proto::RotationGadget::Rotation;

using proto::FilterLogic;
using proto::PlaylistActivateRequest;
using proto::PlaylistChanged;
using proto::PlaylistCreateRequest;
using proto::PlaylistCreateResponse;
using proto::PlaylistDeactivateRequest;
using proto::PlaylistDeleteRequest;
using proto::PlaylistDisplayStatus;
using proto::PlaylistJumpToRequest;
using proto::PlaylistListRequest;
using proto::PlaylistListResponse;
using proto::PlaylistRenameRequest;
using proto::PlaylistSetIntervalRequest;
using proto::PlaylistSetItemsRequest;
using proto::PlaylistSetModeRequest;
using proto::PlaylistStatusRequest;
using proto::PlaylistStatusResponse;
using proto::PlaylistSummary;
using proto::WallpaperFilterRule;
using proto::WallpaperIntFilter;
using proto::WallpaperSortRule;
using proto::WallpaperStringFilter;
using proto::WallpaperTagFilter;
using proto::IntConditionGadget::IntCondition;
using proto::LogicOpGadget::LogicOp;
using proto::PlaylistModeGadget::PlaylistMode;
using proto::SortDirectionGadget::SortDirection;
using proto::StringConditionGadget::StringCondition;
using proto::WallpaperFilterTypeGadget::WallpaperFilterType;
using proto::WallpaperSortKeyGadget::WallpaperSortKey;
} // namespace waywallen::control::v1

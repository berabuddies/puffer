#import <Cocoa/Cocoa.h>
#import <objc/runtime.h>

#include <crt_externs.h>
#include <dispatch/dispatch.h>

#include <algorithm>
#include <chrono>
#include <climits>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <map>
#include <sstream>
#include <string>
#include <utility>

#include "include/cef_app.h"
#include "include/cef_application_mac.h"
#include "include/cef_browser.h"
#include "include/cef_command_line.h"
#include "include/cef_client.h"
#include "include/cef_request_handler.h"
#include "include/cef_version.h"
#include "include/internal/cef_string_wrappers.h"
#include "include/wrapper/cef_helpers.h"
#include "include/wrapper/cef_library_loader.h"

namespace {

constexpr int64_t kMaxPumpDelayMs = 1000 / 30;
constexpr int64_t kPumpDelayPlaceholder = INT_MAX;

static BOOL g_handling_send_event = NO;
static Class g_swizzled_app_class = Nil;
static bool g_framework_loaded = false;
static bool g_initialized = false;
static int g_remote_debugging_port = 0;
static NSTimer* g_message_pump_timer = nil;
static bool g_message_pump_active = false;
static bool g_message_pump_reentrancy_detected = false;
static CefRefPtr<CefApp> g_app;

struct BrowserSlot {
  std::string session_id;
  std::string url;
  std::string title;
  std::string error;
  int64_t updated_at_ms = 0;
  bool loading = false;
  bool creating = false;
  bool closing = false;
  NSView* container = nil;
  CefRefPtr<CefBrowser> browser;
  CefRefPtr<CefClient> client;
};

std::map<std::string, BrowserSlot*> g_slots;

void DeleteSlotLater(BrowserSlot* slot) {
  dispatch_async(dispatch_get_main_queue(), ^{
    delete slot;
  });
}

void WriteError(char* error, size_t error_len, const std::string& message) {
  if (!error || error_len == 0) {
    return;
  }
  const size_t count = std::min(error_len - 1, message.size());
  std::memcpy(error, message.data(), count);
  error[count] = '\0';
}

std::string CefToString(const CefString& value) {
  return value.ToString();
}

int64_t CurrentTimeMs() {
  const auto now = std::chrono::system_clock::now().time_since_epoch();
  return std::chrono::duration_cast<std::chrono::milliseconds>(now).count();
}

void TouchSlot(BrowserSlot* slot) {
  if (slot) {
    slot->updated_at_ms = CurrentTimeMs();
  }
}

std::string JsonEscape(const std::string& value) {
  std::ostringstream out;
  for (const unsigned char ch : value) {
    switch (ch) {
      case '"':
        out << "\\\"";
        break;
      case '\\':
        out << "\\\\";
        break;
      case '\b':
        out << "\\b";
        break;
      case '\f':
        out << "\\f";
        break;
      case '\n':
        out << "\\n";
        break;
      case '\r':
        out << "\\r";
        break;
      case '\t':
        out << "\\t";
        break;
      default:
        if (ch < 0x20) {
          char buffer[7];
          std::snprintf(buffer, sizeof(buffer), "\\u%04x", ch);
          out << buffer;
        } else {
          out << ch;
        }
        break;
    }
  }
  return out.str();
}

BrowserSlot* FindSlot(const std::string& session_id) {
  auto it = g_slots.find(session_id);
  if (it == g_slots.end()) {
    return nullptr;
  }
  return it->second;
}

bool CefDebugEnabled() {
  return std::getenv("PUFFER_BROWSER_DEBUG") != nullptr;
}

void CefDebugLog(const std::string& event, const std::string& details) {
  if (!CefDebugEnabled()) {
    return;
  }
  std::fprintf(stderr, "[puffer-cef-native] %s %s\n", event.c_str(), details.c_str());
  std::fflush(stderr);
}

std::string SlotSummary(const BrowserSlot* slot) {
  if (!slot) {
    return "slot=-";
  }
  std::ostringstream out;
  out << "session_id=" << slot->session_id
      << " browser=" << (slot->browser ? "yes" : "no")
      << " creating=" << (slot->creating ? "true" : "false")
      << " closing=" << (slot->closing ? "true" : "false")
      << " container=" << (slot->container ? "yes" : "no")
      << " slots=" << g_slots.size();
  return out.str();
}

void SetString(cef_string_t* target, const std::string& value) {
  CefString(target).FromString(value);
}

void InstallCefApplicationHooks();
bool LoadCefFramework(const std::string& runtime_root, std::string* error);
void ScheduleMessagePumpWork(int64_t delay_ms);
void RetireBrowserSlotLater(const std::string& session_id);

class PufferCefClient : public CefClient,
                        public CefLifeSpanHandler,
                        public CefDisplayHandler,
                        public CefLoadHandler,
                        public CefRequestHandler {
 public:
  explicit PufferCefClient(std::string session_id)
      : session_id_(std::move(session_id)) {}

  CefRefPtr<CefLifeSpanHandler> GetLifeSpanHandler() override { return this; }
  CefRefPtr<CefDisplayHandler> GetDisplayHandler() override { return this; }
  CefRefPtr<CefLoadHandler> GetLoadHandler() override { return this; }
  CefRefPtr<CefRequestHandler> GetRequestHandler() override { return this; }

#if defined(CEF_VERSION_MAJOR) && CEF_VERSION_MAJOR >= 145
  bool OnBeforePopup(CefRefPtr<CefBrowser> browser,
                     CefRefPtr<CefFrame> frame,
                     int popup_id,
                     const CefString& target_url,
                     const CefString& target_frame_name,
                     CefLifeSpanHandler::WindowOpenDisposition target_disposition,
                     bool user_gesture,
                     const CefPopupFeatures& popupFeatures,
                     CefWindowInfo& windowInfo,
                     CefRefPtr<CefClient>& client,
                     CefBrowserSettings& settings,
                     CefRefPtr<CefDictionaryValue>& extra_info,
                     bool* no_javascript_access) override {
    return HandlePopupInCurrentBrowser(browser, target_url);
  }
#else
  bool OnBeforePopup(CefRefPtr<CefBrowser> browser,
                     CefRefPtr<CefFrame> frame,
                     const CefString& target_url,
                     const CefString& target_frame_name,
                     CefLifeSpanHandler::WindowOpenDisposition target_disposition,
                     bool user_gesture,
                     const CefPopupFeatures& popupFeatures,
                     CefWindowInfo& windowInfo,
                     CefRefPtr<CefClient>& client,
                     CefBrowserSettings& settings,
                     CefRefPtr<CefDictionaryValue>& extra_info,
                     bool* no_javascript_access) override {
    return HandlePopupInCurrentBrowser(browser, target_url);
  }
#endif

  void OnAfterCreated(CefRefPtr<CefBrowser> browser) override {
    if (auto* slot = FindSlot(session_id_)) {
      CefDebugLog("after-created", SlotSummary(slot));
      slot->browser = browser;
      slot->creating = false;
      slot->error.clear();
      slot->loading = false;
      TouchSlot(slot);
      if (slot->closing) {
        if (slot->container) {
          [slot->container setHidden:YES];
        }
        RetireBrowserSlotLater(session_id_);
      }
      return;
    }
    CefDebugLog("after-created-untracked", "session_id=" + session_id_);
    browser->GetHost()->CloseBrowser(true);
    ScheduleMessagePumpWork(0);
  }

  void OnBeforeClose(CefRefPtr<CefBrowser> browser) override {
    auto it = g_slots.find(session_id_);
    if (it == g_slots.end()) {
      CefDebugLog("before-close-untracked", "session_id=" + session_id_);
      return;
    }
    BrowserSlot* slot = it->second;
    CefDebugLog("before-close", SlotSummary(slot));
    if (!slot->browser || !slot->browser->IsSame(browser)) {
      CefDebugLog("before-close-stale", SlotSummary(slot));
      return;
    }
    slot->browser = nullptr;
    slot->creating = false;
    slot->loading = false;
    TouchSlot(slot);
    if (slot->closing) {
      if (slot->container) {
        [slot->container removeFromSuperview];
        slot->container = nil;
      }
      g_slots.erase(it);
      DeleteSlotLater(slot);
      return;
    }
    slot->error = "CEF browser closed unexpectedly";
    TouchSlot(slot);
  }

  void OnBeforeDevToolsPopup(CefRefPtr<CefBrowser> browser,
                             CefWindowInfo& windowInfo,
                             CefRefPtr<CefClient>& client,
                             CefBrowserSettings& settings,
                             CefRefPtr<CefDictionaryValue>& extra_info,
                             bool* use_default_window) override {
    if (use_default_window) {
      *use_default_window = false;
    }
  }

  void OnAddressChange(CefRefPtr<CefBrowser> browser,
                       CefRefPtr<CefFrame> frame,
                       const CefString& url) override {
    if (!frame || !frame->IsMain()) {
      return;
    }
    if (auto* slot = FindSlot(session_id_)) {
      slot->url = CefToString(url);
      TouchSlot(slot);
    }
  }

  void OnTitleChange(CefRefPtr<CefBrowser> browser,
                     const CefString& title) override {
    if (auto* slot = FindSlot(session_id_)) {
      slot->title = CefToString(title);
      TouchSlot(slot);
    }
  }

  void OnLoadingStateChange(CefRefPtr<CefBrowser> browser,
                            bool isLoading,
                            bool canGoBack,
                            bool canGoForward) override {
    if (auto* slot = FindSlot(session_id_)) {
      slot->loading = isLoading;
      TouchSlot(slot);
    }
  }

#if defined(CEF_VERSION_MAJOR) && CEF_VERSION_MAJOR >= 128
  bool OnRenderProcessUnresponsive(
      CefRefPtr<CefBrowser> browser,
      CefRefPtr<CefUnresponsiveProcessCallback> callback) override {
    RecordRenderProcessUnresponsive(browser);
    return true;
  }

  void OnRenderProcessResponsive(CefRefPtr<CefBrowser> browser) override {
    if (auto* slot = FindSlot(session_id_)) {
      if (slot->browser && slot->browser->IsSame(browser) &&
          slot->error == "CEF render process is unresponsive") {
        slot->error.clear();
      }
    }
  }

  void OnRenderProcessTerminated(CefRefPtr<CefBrowser> browser,
                                 CefRequestHandler::TerminationStatus status,
                                 int error_code,
                                 const CefString& error_string) override {
    RecordRenderProcessTerminated(browser, status, error_code, CefToString(error_string));
  }
#else
  void OnRenderProcessTerminated(CefRefPtr<CefBrowser> browser,
                                 CefRequestHandler::TerminationStatus status) override {
    RecordRenderProcessTerminated(browser, status, 0, "");
  }
#endif

  void RecordRenderProcessUnresponsive(CefRefPtr<CefBrowser> browser) {
    if (auto* slot = FindSlot(session_id_)) {
      if (!slot->browser || slot->browser->IsSame(browser)) {
        slot->error = "CEF render process is unresponsive";
        slot->loading = false;
        TouchSlot(slot);
      }
    }
  }

  void RecordRenderProcessTerminated(CefRefPtr<CefBrowser> browser,
                                     CefRequestHandler::TerminationStatus status,
                                     int error_code,
                                     const std::string& detail) {
    if (auto* slot = FindSlot(session_id_)) {
      if (!slot->browser || slot->browser->IsSame(browser)) {
        std::ostringstream message;
        message << "CEF render process terminated";
        if (error_code != 0) {
          message << " with code " << error_code;
        }
        message << " (status " << static_cast<int>(status) << ")";
        if (!detail.empty()) {
          message << ": " << detail;
        }
        slot->error = message.str();
        slot->loading = false;
        TouchSlot(slot);
      }
    }
  }

 private:
  std::string session_id_;

  bool HandlePopupInCurrentBrowser(CefRefPtr<CefBrowser> browser,
                                   const CefString& target_url) {
    const std::string url = CefToString(target_url);
    if (!url.empty() && browser && browser->GetMainFrame()) {
      browser->GetMainFrame()->LoadURL(url);
      if (auto* slot = FindSlot(session_id_)) {
        slot->url = url;
        slot->loading = true;
        slot->error.clear();
        TouchSlot(slot);
      }
    }
    return true;
  }

  IMPLEMENT_REFCOUNTING(PufferCefClient);
  DISALLOW_COPY_AND_ASSIGN(PufferCefClient);
};

class PufferCefApp : public CefApp, public CefBrowserProcessHandler {
 public:
  explicit PufferCefApp(std::string extension_dirs)
      : extension_dirs_(std::move(extension_dirs)) {}

  CefRefPtr<CefBrowserProcessHandler> GetBrowserProcessHandler() override {
    return this;
  }

  void OnBeforeCommandLineProcessing(
      const CefString& process_type,
      CefRefPtr<CefCommandLine> command_line) override {
    if (!process_type.empty() || extension_dirs_.empty()) {
      return;
    }
    command_line->AppendSwitch("enable-extensions");
    command_line->AppendSwitchWithValue("disable-extensions-except",
                                       extension_dirs_);
    command_line->AppendSwitchWithValue("load-extension", extension_dirs_);
  }

  void OnScheduleMessagePumpWork(int64_t delay_ms) override {
    ScheduleMessagePumpWork(delay_ms);
  }

 private:
  std::string extension_dirs_;

  IMPLEMENT_REFCOUNTING(PufferCefApp);
  DISALLOW_COPY_AND_ASSIGN(PufferCefApp);
};

NSRect CefFrameFromCssRect(NSView* content,
                           double x,
                           double y,
                           double width,
                           double height) {
  const NSRect bounds = [content bounds];
  return NSMakeRect(static_cast<CGFloat>(x),
                    bounds.size.height - static_cast<CGFloat>(y + height),
                    static_cast<CGFloat>(width),
                    static_cast<CGFloat>(height));
}

void ResizeBrowserView(BrowserSlot* slot) {
  if (!slot || !slot->browser || !slot->container) {
    return;
  }
  NSView* browser_view =
      CAST_CEF_WINDOW_HANDLE_TO_NSVIEW(slot->browser->GetHost()->GetWindowHandle());
  if (browser_view) {
    [browser_view setFrame:[slot->container bounds]];
  }
  slot->browser->GetHost()->WasResized();
}

bool SetSlotBounds(BrowserSlot* slot,
                   void* ns_window_ptr,
                   double x,
                   double y,
                   double width,
                   double height,
                   std::string* error) {
  if (!slot) {
    *error = "CEF browser slot is missing";
    return false;
  }
  if (!ns_window_ptr) {
    *error = "Tauri NSWindow handle is missing";
    return false;
  }
  NSWindow* window = (__bridge NSWindow*)ns_window_ptr;
  NSView* content = [window contentView];
  if (!content) {
    *error = "Tauri NSWindow content view is missing";
    return false;
  }
  if (!slot->container) {
    slot->container = [[NSView alloc] initWithFrame:NSZeroRect];
    [slot->container setWantsLayer:YES];
    [slot->container setHidden:NO];
    [content addSubview:slot->container positioned:NSWindowAbove relativeTo:nil];
  } else if ([slot->container superview] != content) {
    [slot->container removeFromSuperview];
    [content addSubview:slot->container positioned:NSWindowAbove relativeTo:nil];
  }
  [slot->container setFrame:CefFrameFromCssRect(content, x, y, width, height)];
  [slot->container setHidden:NO];
  ResizeBrowserView(slot);
  return true;
}

void HideOtherSlots(const std::string& session_id) {
  for (const auto& entry : g_slots) {
    if (entry.first != session_id && entry.second && entry.second->container) {
      [entry.second->container setHidden:YES];
    }
  }
}

void RetireBrowserSlotLater(const std::string& session_id) {
  const std::string close_session_id = session_id;
  dispatch_async(dispatch_get_main_queue(), ^{
    BrowserSlot* slot = FindSlot(close_session_id);
    CefDebugLog("close-retired", SlotSummary(slot));
    if (!slot || !slot->closing) {
      return;
    }
    slot->loading = false;
    slot->error.clear();
    if (slot->container) {
      [slot->container setHidden:YES];
      [slot->container setFrame:NSZeroRect];
    }
    TouchSlot(slot);
    ScheduleMessagePumpWork(0);
  });
}

std::string SlotUnavailableMessage(const BrowserSlot* slot) {
  if (!slot) {
    return "CEF browser session is not open";
  }
  if (slot->closing) {
    return "CEF browser session is closing";
  }
  if (!slot->error.empty()) {
    return slot->error;
  }
  return "CEF browser session is not open";
}

std::string SlotStateJson(const BrowserSlot* slot) {
  if (!slot) {
    return "{\"connected\":false,\"url\":\"about:blank\",\"title\":\"\",\"loading\":false,\"updatedAtMs\":0,\"error\":null}";
  }
  std::ostringstream out;
  out << "{\"connected\":"
      << (slot->browser && !slot->closing && slot->error.empty() ? "true" : "false")
      << ",\"url\":\"" << JsonEscape(slot->url.empty() ? "about:blank" : slot->url)
      << "\",\"title\":\"" << JsonEscape(slot->title)
      << "\",\"loading\":" << (slot->loading ? "true" : "false")
      << ",\"updatedAtMs\":" << slot->updated_at_ms
      << ",\"error\":";
  if (slot->error.empty()) {
    out << "null";
  } else {
    out << "\"" << JsonEscape(slot->error) << "\"";
  }
  out
      << ",\"remoteDebuggingPort\":" << g_remote_debugging_port << "}";
  return out.str();
}

int InitializeOnMain(const std::string& runtime_root,
                     const std::string& helper_path,
                     const std::string& cache_path,
                     const std::string& extension_dirs,
                     int remote_debugging_port,
                     std::string* error) {
  if (g_initialized) {
    return 1;
  }
  InstallCefApplicationHooks();

  if (!LoadCefFramework(runtime_root, error)) {
    return 0;
  }

  CefMainArgs main_args(*_NSGetArgc(), *_NSGetArgv());
  CefSettings settings;
  settings.no_sandbox = true;
  settings.external_message_pump = true;
  settings.remote_debugging_port = remote_debugging_port;
  settings.log_severity = LOGSEVERITY_WARNING;

  SetString(&settings.browser_subprocess_path, helper_path);
  SetString(&settings.framework_dir_path,
            runtime_root + "/Chromium Embedded Framework.framework");
  SetString(&settings.main_bundle_path, [[[NSBundle mainBundle] bundlePath] UTF8String]);
  SetString(&settings.resources_dir_path,
            runtime_root + "/Chromium Embedded Framework.framework/Resources");
  SetString(&settings.locales_dir_path,
            runtime_root + "/Chromium Embedded Framework.framework/Resources");
  SetString(&settings.root_cache_path, cache_path);
  SetString(&settings.cache_path, cache_path + "/Default");

  g_app = new PufferCefApp(extension_dirs);
  if (!CefInitialize(main_args, settings, g_app, nullptr)) {
    g_app = nullptr;
    *error = "CefInitialize returned false";
    return 0;
  }
  g_initialized = true;
  g_remote_debugging_port = remote_debugging_port;
  ScheduleMessagePumpWork(0);
  return 1;
}

template <typename Fn>
int RunIntOnMain(Fn fn) {
  if ([NSThread isMainThread]) {
    return fn();
  }
  __block int result = 0;
  dispatch_sync(dispatch_get_main_queue(), ^{
    result = fn();
  });
  return result;
}

template <typename Fn>
std::string RunStringOnMain(Fn fn) {
  if ([NSThread isMainThread]) {
    return fn();
  }
  __block std::string result;
  dispatch_sync(dispatch_get_main_queue(), ^{
    result = fn();
  });
  return result;
}

}  // namespace

@implementation NSApplication (PufferCefAppProtocol)
- (BOOL)isHandlingSendEvent {
  return g_handling_send_event;
}

- (void)setHandlingSendEvent:(BOOL)handlingSendEvent {
  g_handling_send_event = handlingSendEvent;
}

- (void)pufferCefSendEvent:(NSEvent*)event {
  CefScopedSendingEvent sendingEventScoper;
  [self pufferCefSendEvent:event];
}
@end

namespace {

void InstallCefApplicationHooks() {
  NSApplication* app = [NSApplication sharedApplication];
  Class app_class = [app class];
  if (!app_class || g_swizzled_app_class == app_class) {
    return;
  }

  class_addProtocol(app_class, @protocol(CefAppProtocol));
  class_addProtocol([NSApplication class], @protocol(CefAppProtocol));

  Method replacement =
      class_getInstanceMethod([NSApplication class], @selector(pufferCefSendEvent:));
  if (replacement) {
    class_addMethod(app_class, @selector(pufferCefSendEvent:),
                    method_getImplementation(replacement),
                    method_getTypeEncoding(replacement));
  }

  Method original = class_getInstanceMethod(app_class, @selector(sendEvent:));
  replacement = class_getInstanceMethod(app_class, @selector(pufferCefSendEvent:));
  if (original && replacement) {
    method_exchangeImplementations(original, replacement);
    g_swizzled_app_class = app_class;
  }
}

bool LoadCefFramework(const std::string& runtime_root, std::string* error) {
  if (g_framework_loaded) {
    return true;
  }
  if (runtime_root.empty()) {
    *error = "CEF runtime root is empty";
    return false;
  }

  const std::string framework_path =
      runtime_root +
      "/Chromium Embedded Framework.framework/Chromium Embedded Framework";
  NSString* path = [NSString stringWithUTF8String:framework_path.c_str()];
  if (!path || ![[NSFileManager defaultManager] fileExistsAtPath:path]) {
    *error = "CEF framework binary was not found at " + framework_path;
    return false;
  }
  if (!cef_load_library(framework_path.c_str())) {
    *error = "failed to load CEF framework from " + framework_path;
    return false;
  }
  g_framework_loaded = true;
  return true;
}

bool IsMessagePumpTimerPending() {
  return g_message_pump_timer != nil;
}

void KillMessagePumpTimer() {
  if (!g_message_pump_timer) {
    return;
  }
  [g_message_pump_timer invalidate];
  g_message_pump_timer = nil;
}

void DoScheduledMessagePumpWork();

void SetMessagePumpTimer(int64_t delay_ms) {
  const double delay_seconds = static_cast<double>(delay_ms) / 1000.0;
  g_message_pump_timer =
      [NSTimer timerWithTimeInterval:delay_seconds
                              repeats:NO
                                block:^(__unused NSTimer* timer) {
                                  KillMessagePumpTimer();
                                  DoScheduledMessagePumpWork();
                                }];
  NSRunLoop* run_loop = [NSRunLoop currentRunLoop];
  [run_loop addTimer:g_message_pump_timer forMode:NSRunLoopCommonModes];
  [run_loop addTimer:g_message_pump_timer forMode:NSEventTrackingRunLoopMode];
}

void DoScheduledMessagePumpWork() {
  if (!g_initialized) {
    return;
  }
  if (g_message_pump_active) {
    g_message_pump_reentrancy_detected = true;
    return;
  }

  g_message_pump_reentrancy_detected = false;
  g_message_pump_active = true;
  CefDoMessageLoopWork();
  g_message_pump_active = false;

  if (g_message_pump_reentrancy_detected) {
    ScheduleMessagePumpWork(0);
  } else if (!IsMessagePumpTimerPending()) {
    ScheduleMessagePumpWork(kPumpDelayPlaceholder);
  }
}

void HandleScheduledMessagePumpWork(int64_t delay_ms) {
  if (delay_ms == kPumpDelayPlaceholder && IsMessagePumpTimerPending()) {
    return;
  }

  KillMessagePumpTimer();

  if (delay_ms <= 0) {
    DoScheduledMessagePumpWork();
    return;
  }

  if (delay_ms > kMaxPumpDelayMs) {
    delay_ms = kMaxPumpDelayMs;
  }
  SetMessagePumpTimer(delay_ms);
}

void ScheduleMessagePumpWork(int64_t delay_ms) {
  dispatch_async(dispatch_get_main_queue(), ^{
    if (g_initialized) {
      HandleScheduledMessagePumpWork(delay_ms);
    }
  });
}

}  // namespace

extern "C" int puffer_cef_initialize(const char* runtime_root,
                                      const char* helper_path,
                                      const char* cache_path,
                                      const char* extension_dirs,
                                      int remote_debugging_port,
                                      char* error,
                                      size_t error_len) {
  const std::string root = runtime_root ? runtime_root : "";
  const std::string helper = helper_path ? helper_path : "";
  const std::string cache = cache_path ? cache_path : "";
  const std::string extensions = extension_dirs ? extension_dirs : "";
  std::string message;
  int result = RunIntOnMain([&]() {
    return InitializeOnMain(root, helper, cache, extensions, remote_debugging_port, &message);
  });
  if (!result) {
    WriteError(error, error_len, message);
  }
  return result;
}

extern "C" int puffer_cef_open(const char* session_id,
                                void* ns_window,
                                double x,
                                double y,
                                double width,
                                double height,
                                const char* url,
                                char* error,
                                size_t error_len) {
  const std::string id = session_id ? session_id : "";
  const std::string target_url = url && std::strlen(url) > 0 ? url : "about:blank";
  std::string message;
  int result = RunIntOnMain([&]() {
    if (!g_initialized) {
      message = "CEF is not initialized";
      return 0;
    }
    BrowserSlot* slot = FindSlot(id);
    if (!slot) {
      slot = new BrowserSlot();
      slot->session_id = id;
      slot->url = target_url;
      slot->client = new PufferCefClient(id);
      g_slots[id] = slot;
      TouchSlot(slot);
    } else if (slot->closing) {
      message = "CEF browser session is closing";
      CefDebugLog("open-closing-slot", SlotSummary(slot));
      return 0;
    }
    if (slot->browser && !slot->error.empty()) {
      slot->browser->GetHost()->CloseBrowser(true);
      slot->browser = nullptr;
    }
    slot->closing = false;
    slot->error.clear();
    TouchSlot(slot);
    HideOtherSlots(id);
    if (!SetSlotBounds(slot, ns_window, x, y, width, height, &message)) {
      slot->error = message;
      TouchSlot(slot);
      return 0;
    }
    if (!slot->browser) {
      if (!slot->creating) {
        CefWindowInfo window_info;
        window_info.SetAsChild((__bridge CefWindowHandle)slot->container,
                               CefRect(0, 0, static_cast<int>(width),
                                       static_cast<int>(height)));
        CefBrowserSettings settings;
        slot->loading = true;
        slot->creating = true;
        TouchSlot(slot);
        if (!CefBrowserHost::CreateBrowser(window_info, slot->client, target_url,
                                           settings, nullptr, nullptr)) {
          slot->creating = false;
          slot->loading = false;
          message = "CefBrowserHost::CreateBrowser returned false";
          slot->error = message;
          TouchSlot(slot);
          return 0;
        }
      }
    } else {
      [slot->container setHidden:NO];
      ResizeBrowserView(slot);
    }
    ScheduleMessagePumpWork(0);
    return 1;
  });
  if (!result) {
    WriteError(error, error_len, message);
  }
  return result;
}

extern "C" int puffer_cef_resize(const char* session_id,
                                  void* ns_window,
                                  double x,
                                  double y,
                                  double width,
                                  double height,
                                  char* error,
                                  size_t error_len) {
  const std::string id = session_id ? session_id : "";
  std::string message;
  int result = RunIntOnMain([&]() {
    BrowserSlot* slot = FindSlot(id);
    if (!slot || slot->closing || !slot->browser || !slot->error.empty()) {
      message = SlotUnavailableMessage(slot);
      return 0;
    }
    if (!SetSlotBounds(slot, ns_window, x, y, width, height, &message)) {
      slot->error = message;
      TouchSlot(slot);
      return 0;
    }
    slot->error.clear();
    TouchSlot(slot);
    ScheduleMessagePumpWork(0);
    return 1;
  });
  if (!result) {
    WriteError(error, error_len, message);
  }
  return result;
}

extern "C" int puffer_cef_navigate(const char* session_id,
                                    const char* url,
                                    char* error,
                                    size_t error_len) {
  const std::string id = session_id ? session_id : "";
  const std::string target_url = url && std::strlen(url) > 0 ? url : "about:blank";
  std::string message;
  int result = RunIntOnMain([&]() {
    BrowserSlot* slot = FindSlot(id);
    if (!slot || slot->closing || !slot->browser || !slot->error.empty()) {
      message = SlotUnavailableMessage(slot);
      if (slot) {
        slot->error = message;
      }
      return 0;
    }
    slot->url = target_url;
    slot->loading = true;
    slot->error.clear();
    TouchSlot(slot);
    slot->browser->GetMainFrame()->LoadURL(target_url);
    ScheduleMessagePumpWork(0);
    return 1;
  });
  if (!result) {
    WriteError(error, error_len, message);
  }
  return result;
}

extern "C" int puffer_cef_reload(const char* session_id,
                                  char* error,
                                  size_t error_len) {
  const std::string id = session_id ? session_id : "";
  std::string message;
  int result = RunIntOnMain([&]() {
    BrowserSlot* slot = FindSlot(id);
    if (!slot || slot->closing || !slot->browser || !slot->error.empty()) {
      message = SlotUnavailableMessage(slot);
      if (slot) {
        slot->error = message;
      }
      return 0;
    }
    slot->loading = true;
    slot->error.clear();
    TouchSlot(slot);
    slot->browser->Reload();
    ScheduleMessagePumpWork(0);
    return 1;
  });
  if (!result) {
    WriteError(error, error_len, message);
  }
  return result;
}

extern "C" int puffer_cef_history(const char* session_id,
                                   int direction,
                                   char* error,
                                   size_t error_len) {
  const std::string id = session_id ? session_id : "";
  std::string message;
  int result = RunIntOnMain([&]() {
    BrowserSlot* slot = FindSlot(id);
    if (!slot || slot->closing || !slot->browser || !slot->error.empty()) {
      message = SlotUnavailableMessage(slot);
      if (slot) {
        slot->error = message;
      }
      return 0;
    }
    slot->error.clear();
    TouchSlot(slot);
    if (direction < 0) {
      if (!slot->browser->CanGoBack()) {
        return 1;
      }
      slot->loading = true;
      TouchSlot(slot);
      slot->browser->GoBack();
    } else {
      if (!slot->browser->CanGoForward()) {
        return 1;
      }
      slot->loading = true;
      TouchSlot(slot);
      slot->browser->GoForward();
    }
    ScheduleMessagePumpWork(0);
    return 1;
  });
  if (!result) {
    WriteError(error, error_len, message);
  }
  return result;
}

extern "C" int puffer_cef_close(const char* session_id,
                                 char* error,
                                 size_t error_len) {
  const std::string id = session_id ? session_id : "";
  std::string message;
  int result = RunIntOnMain([&]() {
    BrowserSlot* slot = FindSlot(id);
    if (!slot) {
      return 1;
    }
    slot->closing = true;
    if (slot->browser) {
      if (slot->container) {
        [slot->container setHidden:YES];
      }
      CefDebugLog("close-request", SlotSummary(slot));
      RetireBrowserSlotLater(id);
      ScheduleMessagePumpWork(0);
      return 1;
    }
    if (slot->creating) {
      if (slot->container) {
        [slot->container setHidden:YES];
      }
      CefDebugLog("close-pending-create", SlotSummary(slot));
      ScheduleMessagePumpWork(0);
      return 1;
    }
    if (slot->container) {
      [slot->container removeFromSuperview];
      slot->container = nil;
    }
    g_slots.erase(id);
    delete slot;
    ScheduleMessagePumpWork(0);
    return 1;
  });
  if (!result) {
    WriteError(error, error_len, message);
  }
  return result;
}

extern "C" int puffer_cef_hide(const char* session_id,
                                char* error,
                                size_t error_len) {
  const std::string id = session_id ? session_id : "";
  std::string message;
  int result = RunIntOnMain([&]() {
    BrowserSlot* slot = FindSlot(id);
    if (!slot) {
      return 1;
    }
    if (slot->container) {
      [slot->container setHidden:YES];
    }
    ScheduleMessagePumpWork(0);
    return 1;
  });
  if (!result) {
    WriteError(error, error_len, message);
  }
  return result;
}

extern "C" char* puffer_cef_state_json(const char* session_id) {
  const std::string id = session_id ? session_id : "";
  std::string json = RunStringOnMain([&]() {
    return SlotStateJson(FindSlot(id));
  });
  char* out = static_cast<char*>(std::malloc(json.size() + 1));
  if (!out) {
    return nullptr;
  }
  std::memcpy(out, json.c_str(), json.size() + 1);
  return out;
}

extern "C" void puffer_cef_free_string(char* value) {
  std::free(value);
}

#import <Cocoa/Cocoa.h>
#import <Carbon/Carbon.h>
#import <IOKit/hidsystem/IOLLEvent.h>
#import <InputMethodKit/InputMethodKit.h>

// A synthetic phrase keeps the prototype independent of microphone and model setup.
@interface VoaiProbeController : IMKInputController
@property NSRange capturedRange;
@end
@implementation VoaiProbeController
- (void)activateServer:(id)sender {
    [super activateServer:sender];
    NSDistributedNotificationCenter *center = NSDistributedNotificationCenter.defaultCenter;
    [center removeObserver:self];
    self.capturedRange = [(id<IMKTextInput>)sender selectedRange];
    [center addObserver:self selector:@selector(commitProbe:) name:@"app.voai.inputmethod.probe.commit" object:nil];
}
- (void)deactivateServer:(id)sender {
    [NSDistributedNotificationCenter.defaultCenter removeObserver:self];
    [super deactivateServer:sender];
}
- (void)commitProbe:(NSNotification *)note {
    id<IMKTextInput> client = [self client];
    if (![client.bundleIdentifier isEqualToString:@"dev.zed.Zed"]) return;
    if (![note.object isKindOfClass:NSString.class] || ![note.object hasPrefix:@"Voai IMK tmp."]) return;
    NSString *marker = [(NSString *)note.object stringByAppendingString:@"\n"];
    NSString *prefix = [client attributedSubstringFromRange:NSMakeRange(0,marker.length)].string;
    if (![prefix isEqualToString:marker]) return;
    NSRange range = [client selectedRange];
    if (range.location == NSNotFound || !NSEqualRanges(range,self.capturedRange)) return;
    [client insertText:@"Проверка Voai 🎙" replacementRange:range];
}
- (BOOL)handleEvent:(NSEvent *)event client:(id)sender { return NO; }
@end

static NSString *const identifier = @"app.voai.inputmethod.probe";
static TISInputSourceRef findSource(NSString *sourceID) {
    NSDictionary *filter = @{(__bridge NSString *)kTISPropertyInputSourceID:sourceID};
    NSArray *sources = CFBridgingRelease(TISCreateInputSourceList((__bridge CFDictionaryRef)filter,true));
    if (!sources.count) return NULL;
    return (TISInputSourceRef)CFRetain((__bridge CFTypeRef)sources.firstObject);
}
// Restrict automated events to the disposable document created by the launcher.
static BOOL demoWindow(NSString *name, pid_t *pid) {
    NSRunningApplication *front = NSWorkspace.sharedWorkspace.frontmostApplication;
    if (![front.bundleIdentifier isEqualToString:@"dev.zed.Zed"]) return NO;
    AXUIElementRef app = AXUIElementCreateApplication(front.processIdentifier);
    CFTypeRef window = NULL, title = NULL;
    AXUIElementCopyAttributeValue(app,kAXFocusedWindowAttribute,&window);
    if (window) AXUIElementCopyAttributeValue((AXUIElementRef)window,kAXTitleAttribute,&title);
    BOOL match = title && CFGetTypeID(title)==CFStringGetTypeID() &&
                 [(__bridge NSString *)title containsString:name];
    if (title) CFRelease(title);
    if (window) CFRelease(window);
    CFRelease(app);
    *pid = front.processIdentifier;
    return match;
}

static BOOL held(CGKeyCode key) {
    if (key == kVK_RightOption) return (CGEventSourceFlagsState(kCGEventSourceStateCombinedSessionState) & NX_DEVICERALTKEYMASK) != 0;
    return CGEventSourceKeyState(kCGEventSourceStateCombinedSessionState,key);
}

static int demo(NSString *name, BOOL automatic) {
    fprintf(stderr,"In the new Zed document, hold right Option for one second, then release. Esc cancels.\n");
    pid_t pid = 0;
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:60];
    while ((!automatic && !held(kVK_RightOption)) || !demoWindow(name,&pid)) {
        if (deadline.timeIntervalSinceNow <= 0 || held(kVK_Escape)) return 1;
        [NSThread sleepForTimeInterval:0.03];
    }
    TISInputSourceRef old = TISCopyCurrentKeyboardInputSource();
    TISInputSourceRef source = findSource(identifier);
    if (!old || !source) return 1;
    int result = 1;
    if (TISEnableInputSource(source) == noErr && TISSelectInputSource(source) == noErr) {
        if (automatic) [NSThread sleepForTimeInterval:1];
        BOOL cancelled = NO;
        while (held(kVK_RightOption)) {
            if (held(kVK_Escape) || deadline.timeIntervalSinceNow <= 0 || !demoWindow(name,&pid)) {
                cancelled = YES;
                break;
            }
            [NSThread sleepForTimeInterval:0.03];
        }
        TISInputSourceRef current = TISCopyCurrentKeyboardInputSource();
        if (current && CFEqual(current,source)) {
            if (!cancelled && !held(kVK_Escape) && demoWindow(name,&pid)) {
                [NSDistributedNotificationCenter.defaultCenter postNotificationName:@"app.voai.inputmethod.probe.commit"
                    object:name userInfo:nil deliverImmediately:YES];
                [NSThread sleepForTimeInterval:0.3];
            }
            result = TISSelectInputSource(old) != noErr;
        }
        if (current) CFRelease(current);
    }
    if (TISDisableInputSource(source) != noErr) result = 1;
    CFRelease(old);
    CFRelease(source);
    return result;
}
int main(int argc, const char **argv) {
    @autoreleasepool {
        if (argc == 3 && [@(argv[1]) isEqual:@"demo"]) return demo(@(argv[2]),getenv("VOAI_IMK_SMOKE") != NULL);
        if (argc == 2) {
            NSString *action = @(argv[1]);
            if ([action isEqual:@"register"]) {
                OSStatus status = TISRegisterInputSource((__bridge CFURLRef)NSBundle.mainBundle.bundleURL);
                if (status) { fprintf(stderr,"Registration: %d\n",(int)status); return 1; }
            }
            TISInputSourceRef source = findSource(identifier);
            if (!source) { fprintf(stderr,"Input source unavailable\n"); return 1; }
            OSStatus status = paramErr;
            if ([action isEqual:@"register"]) status = noErr;
            if ([action isEqual:@"disable"]) status = TISDisableInputSource(source);
            CFRelease(source);
            fprintf(stderr,"TIS status: %d\n",(int)status);
            return status != noErr;
        }
        [NSApplication sharedApplication];
        __attribute__((objc_precise_lifetime)) IMKServer *server = [[IMKServer alloc] initWithName:@"app.voai.inputmethod.probe_Connection" bundleIdentifier:identifier];
        if (!server) return 1;
        [NSApp run];
    }
}

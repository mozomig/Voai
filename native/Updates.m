// Called exclusively from the GPUI main thread. The framework is loaded from
// this signed bundle so command-line workers and cargo builds do not load it.
#import <Foundation/Foundation.h>
#ifdef VOAI_WITH_SPARKLE
#import <Sparkle/Sparkle.h>

@interface VoaiUpdateDelegate : NSObject <SPUUpdaterDelegate>
@property BOOL busy;
@property(copy) void (^resumeInstall)(void);
@end

@implementation VoaiUpdateDelegate
- (BOOL)updater:(SPUUpdater *)updater shouldPostponeRelaunchForUpdate:(SUAppcastItem *)item
    untilInvokingBlock:(void (^)(void))install {
    if (!self.busy) return NO;
    self.resumeInstall = install;
    return YES;
}
@end

static SPUStandardUpdaterController *controller;
static VoaiUpdateDelegate *delegate;

bool voai_updates_start(void) {
    NSBundle *app = NSBundle.mainBundle;
    if (![app objectForInfoDictionaryKey:@"SUFeedURL"]) return false;
    NSURL *url = [app.privateFrameworksURL URLByAppendingPathComponent:@"Sparkle.framework"];
    NSBundle *framework = [NSBundle bundleWithURL:url];
    if (![framework loadAndReturnError:NULL]) return false;
    Class cls = [framework classNamed:@"SPUStandardUpdaterController"];
    delegate = [VoaiUpdateDelegate new];
    controller = [[cls alloc] initWithStartingUpdater:NO updaterDelegate:delegate userDriverDelegate:nil];
    return [controller.updater startUpdater:NULL];
}
void voai_updates_check(void) { [controller checkForUpdates:nil]; }
bool voai_updates_auto(void) { return controller.updater.automaticallyChecksForUpdates; }
void voai_updates_set_auto(bool enabled) { controller.updater.automaticallyChecksForUpdates = enabled; }
void voai_updates_set_busy(bool busy) {
    delegate.busy = busy;
    if (!busy && delegate.resumeInstall) {
        void (^resume)(void) = delegate.resumeInstall;
        delegate.resumeInstall = nil;
        resume();
    }
}
#else
bool voai_updates_start(void) { return false; }
void voai_updates_check(void) {}
bool voai_updates_auto(void) { return false; }
void voai_updates_set_auto(bool enabled) {}
void voai_updates_set_busy(bool busy) {}
#endif

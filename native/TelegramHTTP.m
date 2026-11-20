// URLSession handles system proxies (including PAC) and Network Extension VPN routing.
// The caller owns the output sink until completion. No URL or NSError crosses into logs.
#import <Foundation/Foundation.h>
#import <objc/runtime.h>
#import <Network/NSURLSession+Network.h>

typedef bool (*WriteChunk)(void *, const uint8_t *, size_t);

typedef struct {
    int32_t error;
    int32_t status;
    uint64_t retryAfter;
} HTTPResult;

@interface VoaiHTTPSink : NSObject
@property (nonatomic, strong) dispatch_semaphore_t done;
@property void *context;
@property WriteChunk writeChunk;
@property uint64_t limit;
@property uint64_t received;
@property HTTPResult result;
@end
@implementation VoaiHTTPSink
@end

static char sinkKey;
static const int32_t tooLarge = 1;
static const int32_t invalidResponse = 2;

@interface VoaiTelegramHTTP : NSObject <NSURLSessionDataDelegate>
@property (nonatomic, strong) NSURLSession *session;
@property BOOL stopped;
@end

@implementation VoaiTelegramHTTP
- (void)URLSession:(NSURLSession *)session task:(NSURLSessionTask *)task
    willPerformHTTPRedirection:(NSHTTPURLResponse *)response
    newRequest:(NSURLRequest *)request
    completionHandler:(void (^)(NSURLRequest *))completion {
    completion(nil);
}

- (void)URLSession:(NSURLSession *)session dataTask:(NSURLSessionDataTask *)task
didReceiveResponse:(NSURLResponse *)response completionHandler:(void (^)(NSURLSessionResponseDisposition))completion {
    VoaiHTTPSink *sink = objc_getAssociatedObject(task, &sinkKey);
    HTTPResult result = {0};
    if (![response isKindOfClass:NSHTTPURLResponse.class]) {
        result.error = invalidResponse;
    } else {
        NSHTTPURLResponse *http = (NSHTTPURLResponse *)response;
        result.status = (int32_t)http.statusCode;
        result.retryAfter = MAX(0, [[http valueForHTTPHeaderField:@"Retry-After"] longLongValue]);
        if (response.expectedContentLength > 0 && (uint64_t)response.expectedContentLength > sink.limit)
            result.error = tooLarge;
    }
    sink.result = result;
    completion(result.error ? NSURLSessionResponseCancel : NSURLSessionResponseAllow);
}

- (void)URLSession:(NSURLSession *)session dataTask:(NSURLSessionDataTask *)task didReceiveData:(NSData *)data {
    VoaiHTTPSink *sink = objc_getAssociatedObject(task, &sinkKey);
    if (sink.result.error) return;
    HTTPResult result = sink.result;
    if (data.length > sink.limit - sink.received) {
        result.error = tooLarge;
    } else if (data.length && !sink.writeChunk(sink.context, data.bytes, data.length)) {
        result.error = NSURLErrorCancelled;
    } else {
        sink.received += data.length;
        return;
    }
    sink.result = result;
    [task cancel];
}

- (void)URLSession:(NSURLSession *)session task:(NSURLSessionTask *)task didCompleteWithError:(NSError *)error {
    VoaiHTTPSink *sink = objc_getAssociatedObject(task, &sinkKey);
    if (error && !sink.result.error) {
        HTTPResult result = sink.result;
        result.error = [error.domain isEqualToString:NSURLErrorDomain] ? (int32_t)error.code : NSURLErrorUnknown;
        sink.result = result;
    }
    dispatch_semaphore_signal(sink.done);
}
@end

void *voai_http_create(uint16_t proxyPort) {
    @autoreleasepool {
        VoaiTelegramHTTP *client = [VoaiTelegramHTTP new];
        NSURLSessionConfiguration *config = NSURLSessionConfiguration.ephemeralSessionConfiguration;
        config.URLCache = nil;
        config.HTTPCookieStorage = nil;
        config.URLCredentialStorage = nil;
        config.requestCachePolicy = NSURLRequestReloadIgnoringLocalCacheData;
        config.timeoutIntervalForResource = 180;
        // By default CFNetwork uses the macOS route and system proxy settings.
        if (proxyPort) {
            NSString *port = [NSString stringWithFormat:@"%u", (unsigned)proxyPort];
            nw_endpoint_t local = nw_endpoint_create_host("127.0.0.1", port.UTF8String);
            nw_proxy_config_t proxy = nw_proxy_config_create_socksv5(local);
            nw_proxy_config_set_failover_allowed(proxy, false);
            config.proxyConfigurations = @[proxy];
        }
        client.session = [NSURLSession sessionWithConfiguration:config delegate:client delegateQueue:nil];
        return (void *)CFBridgingRetain(client);
    }
}

void voai_http_stop(void *handle) {
    @autoreleasepool {
        VoaiTelegramHTTP *client = (__bridge VoaiTelegramHTTP *)handle;
        @synchronized (client) {
            if (client.stopped) return;
            client.stopped = YES;
            [client.session invalidateAndCancel];
        }
    }
}

void voai_http_release(void *handle) {
    voai_http_stop(handle);
    CFRelease(handle);
}

HTTPResult voai_http_request(void *handle, const char *url, const uint8_t *body, size_t bodyLen,
                             double timeout, uint64_t limit, WriteChunk writeChunk, void *context) {
    @autoreleasepool {
        VoaiTelegramHTTP *client = (__bridge VoaiTelegramHTTP *)handle;
        NSURL *endpoint = [NSURL URLWithString:[NSString stringWithUTF8String:url]];
        if (!endpoint) return (HTTPResult){.error = NSURLErrorBadURL};
        NSMutableURLRequest *request = [NSMutableURLRequest requestWithURL:endpoint];
        request.timeoutInterval = timeout;
        [request setValue:@"identity" forHTTPHeaderField:@"Accept-Encoding"];
        if (body) {
            request.HTTPMethod = @"POST";
            request.HTTPBody = [NSData dataWithBytes:body length:bodyLen];
            [request setValue:@"application/json" forHTTPHeaderField:@"Content-Type"];
        }
        VoaiHTTPSink *sink = [VoaiHTTPSink new];
        sink.done = dispatch_semaphore_create(0);
        sink.limit = limit;
        sink.writeChunk = writeChunk;
        sink.context = context;
        NSURLSessionDataTask *task;
        @synchronized (client) {
            if (client.stopped) return (HTTPResult){.error = NSURLErrorCancelled};
            task = [client.session dataTaskWithRequest:request];
            objc_setAssociatedObject(task, &sinkKey, sink, OBJC_ASSOCIATION_RETAIN_NONATOMIC);
            [task resume];
        }
        // Bound the whole exchange even when a peer keeps sending a slow trickle.
        if (dispatch_semaphore_wait(sink.done, dispatch_time(DISPATCH_TIME_NOW, (int64_t)(timeout * NSEC_PER_SEC)))) {
            [task cancel];
            dispatch_semaphore_wait(sink.done, DISPATCH_TIME_FOREVER);
            return (HTTPResult){.error = NSURLErrorTimedOut};
        }
        return sink.result;
    }
}

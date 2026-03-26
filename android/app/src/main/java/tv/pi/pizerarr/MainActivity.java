package tv.pi.pizerarr;

import android.app.Activity;
import android.content.Intent;
import android.net.Uri;
import android.os.Bundle;
import android.view.KeyEvent;
import android.view.View;
import android.view.Window;
import android.view.WindowManager;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.SslErrorHandler;
import android.webkit.WebViewClient;
import android.net.http.SslError;

public class MainActivity extends Activity {
    private WebView webView;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        requestWindowFeature(Window.FEATURE_NO_TITLE);
        getWindow().setFlags(
            WindowManager.LayoutParams.FLAG_FULLSCREEN,
            WindowManager.LayoutParams.FLAG_FULLSCREEN
        );

        webView = new WebView(this);
        setContentView(webView);

        WebSettings settings = webView.getSettings();
        settings.setJavaScriptEnabled(true);
        settings.setDomStorageEnabled(true);
        settings.setMediaPlaybackRequiresUserGesture(false);
        settings.setMixedContentMode(WebSettings.MIXED_CONTENT_ALWAYS_ALLOW);
        settings.setUserAgentString(settings.getUserAgentString() + " pizerarr-tv/1.0");

        webView.setWebViewClient(new WebViewClient() {
            @Override
            public void onReceivedSslError(WebView view, SslErrorHandler handler, SslError error) {
                // Accept self-signed cert for local pizerarr server
                handler.proceed();
            }

            @Override
            public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
                String url = request.getUrl().toString();

                // Handle intent:// URLs — launch external apps (VLC etc)
                if (url.startsWith("intent:")) {
                    try {
                        Intent intent = Intent.parseUri(url, Intent.URI_INTENT_SCHEME);
                        if (intent.resolveActivity(getPackageManager()) != null) {
                            startActivity(intent);
                        } else {
                            // App not installed — try opening the raw video URL with any player
                            String videoUrl = url.substring(7, url.indexOf("#Intent"));
                            Intent fallback = new Intent(Intent.ACTION_VIEW);
                            fallback.setDataAndType(Uri.parse(videoUrl), "video/*");
                            startActivity(fallback);
                        }
                    } catch (Exception e) {
                        e.printStackTrace();
                    }
                    return true;
                }

                // Handle vlc:// URLs
                if (url.startsWith("vlc://")) {
                    try {
                        Intent intent = new Intent(Intent.ACTION_VIEW);
                        String videoUrl = url.substring(6);
                        intent.setDataAndType(Uri.parse(videoUrl), "video/*");
                        intent.setPackage("org.videolan.vlc");
                        startActivity(intent);
                    } catch (Exception e) {
                        // VLC not installed, try any video player
                        Intent fallback = new Intent(Intent.ACTION_VIEW);
                        fallback.setDataAndType(Uri.parse(url.substring(6)), "video/*");
                        startActivity(fallback);
                    }
                    return true;
                }

                return false;
            }
        });

        webView.setWebChromeClient(new WebChromeClient() {
            @Override
            public boolean onConsoleMessage(android.webkit.ConsoleMessage msg) {
                android.util.Log.d("pizerarr", msg.message() + " [" + msg.sourceId() + ":" + msg.lineNumber() + "]");
                return true;
            }
        });

        webView.setSystemUiVisibility(
            View.SYSTEM_UI_FLAG_FULLSCREEN |
            View.SYSTEM_UI_FLAG_HIDE_NAVIGATION |
            View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
        );

        webView.requestFocus();
        webView.loadUrl("https://pizr.duckdns.org");
    }

    @Override
    public boolean onKeyDown(int keyCode, KeyEvent event) {
        if (keyCode == KeyEvent.KEYCODE_BACK) {
            // Let JS handle back — close player/modals before leaving
            if (webView != null) {
                webView.evaluateJavascript("handleBack()", result -> {
                    if ("false".equals(result)) {
                        // Nothing to close, let system handle it (but don't exit)
                        // Only exit if pressed twice quickly
                    }
                });
                return true;
            }
        }
        if (webView != null) {
            webView.requestFocus();
        }
        return super.onKeyDown(keyCode, event);
    }

    @Override
    public void onBackPressed() {
        // Don't call super — we handle back in onKeyDown via JS
    }

    @Override
    protected void onResume() {
        super.onResume();
        if (webView != null) webView.onResume();
    }

    @Override
    protected void onPause() {
        if (webView != null) webView.onPause();
        super.onPause();
    }
}

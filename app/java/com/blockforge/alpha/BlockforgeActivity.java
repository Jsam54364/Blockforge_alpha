package com.blockforge.alpha;

import android.app.NativeActivity;
import android.os.Build;
import android.os.Bundle;
import android.view.DisplayCutout;
import android.view.View;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;

/**
 * The game's activity: Android's NativeActivity (which runs the Rust code) plus the few
 * things that can only be done from Java, on the UI thread:
 *
 * - immersive mode: the status and navigation bars are hidden, and come back for a moment
 *   when swiped in from the edge of the screen (hidden again when the game has focus again);
 * - the game draws under a camera cut-out too (landscape phones put it on a short edge);
 * - the screen stays on while the game is open;
 * - the safe area: how far the system bars and cut-out still reach into each edge, which the
 *   Rust side reads (safeInsets, through JNI) to keep the HUD clear of them.
 *
 * Built into classes.dex by the Android workflow (cargo-apk does not compile Java).
 */
public class BlockforgeActivity extends NativeActivity {
    /** Left, top, right, bottom in pixels; -1 until the first insets arrive. */
    private static volatile int[] safe = {-1, -1, -1, -1};

    /** The safe area (see above). Called by the Rust side from its own thread. */
    public static int[] safeInsets() {
        return safe;
    }

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        Window w = getWindow();
        w.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        if (Build.VERSION.SDK_INT >= 28) {
            WindowManager.LayoutParams lp = w.getAttributes();
            lp.layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
            w.setAttributes(lp);
        }
        w.getDecorView().setOnApplyWindowInsetsListener(new View.OnApplyWindowInsetsListener() {
            @Override
            public WindowInsets onApplyWindowInsets(View v, WindowInsets insets) {
                record(insets);
                return v.onApplyWindowInsets(insets);
            }
        });
        hideSystemBars();
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) {
            hideSystemBars();
        }
    }

    @SuppressWarnings("deprecation")
    private void hideSystemBars() {
        Window w = getWindow();
        if (Build.VERSION.SDK_INT >= 30) {
            w.setDecorFitsSystemWindows(false);
            WindowInsetsController c = w.getInsetsController();
            if (c != null) {
                c.setSystemBarsBehavior(WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
                c.hide(WindowInsets.Type.systemBars());
            }
        } else {
            w.getDecorView().setSystemUiVisibility(
                    View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                            | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                            | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                            | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                            | View.SYSTEM_UI_FLAG_FULLSCREEN
                            | View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY);
        }
    }

    @SuppressWarnings("deprecation")
    private static void record(WindowInsets in) {
        int l, t, r, b;
        if (Build.VERSION.SDK_INT >= 30) {
            android.graphics.Insets i = in.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.displayCutout());
            l = i.left;
            t = i.top;
            r = i.right;
            b = i.bottom;
        } else {
            l = in.getSystemWindowInsetLeft();
            t = in.getSystemWindowInsetTop();
            r = in.getSystemWindowInsetRight();
            b = in.getSystemWindowInsetBottom();
            if (Build.VERSION.SDK_INT >= 28) {
                DisplayCutout c = in.getDisplayCutout();
                if (c != null) {
                    l = Math.max(l, c.getSafeInsetLeft());
                    t = Math.max(t, c.getSafeInsetTop());
                    r = Math.max(r, c.getSafeInsetRight());
                    b = Math.max(b, c.getSafeInsetBottom());
                }
            }
        }
        safe = new int[] {l, t, r, b};
    }
}

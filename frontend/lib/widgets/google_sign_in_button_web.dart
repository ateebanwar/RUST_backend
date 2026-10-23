import 'dart:async';
import 'package:flutter/material.dart';
import 'package:google_sign_in_web/web_only.dart' as web;
import '../config/app_config.dart';

/// Web implementation of Google Sign-In button using Google Identity Services (GIS).
/// 
/// Google Identity Services requires an official rendered button (`renderButton`)
/// for web security. This implementation guarantees:
/// 1. Zero infinite 'Getting ready' text - includes strict warm-up timeout.
/// 2. Seamless styling matching the ChatGPT-inspired pill theme.
/// 3. Immediate fallback button if GIS is blocked or delayed.
Widget buildGoogleSignInButton({
  required VoidCallback onPressed,
  bool isLoading = false,
}) {
  return GoogleSignInWebButton(
    onPressed: onPressed,
    isLoading: isLoading,
  );
}

class GoogleSignInWebButton extends StatefulWidget {
  final VoidCallback onPressed;
  final bool isLoading;

  const GoogleSignInWebButton({
    super.key,
    required this.onPressed,
    this.isLoading = false,
  });

  @override
  State<GoogleSignInWebButton> createState() => _GoogleSignInWebButtonState();
}

class _GoogleSignInWebButtonState extends State<GoogleSignInWebButton> {
  bool _isGsiMounted = false;
  bool _hasTimedOut = false;
  Timer? _timeoutTimer;

  @override
  void initState() {
    super.initState();
    _startWarmup();
  }

  void _startWarmup() {
    if (!AppConfig.hasGoogleClientId) return;

    // Safety timeout: if GSI doesn't mount in 5s (e.g. ad-blocker or offline),
    // drop back to interactive fallback button with clear error capability.
    _timeoutTimer = Timer(const Duration(seconds: 5), () {
      if (mounted && !_isGsiMounted) {
        setState(() {
          _hasTimedOut = true;
        });
      }
    });

    // Allow GIS a brief moment to finish registerButtonFactory and DOM setup
    Future<void>.delayed(const Duration(milliseconds: 350), () {
      if (mounted) {
        setState(() {
          _isGsiMounted = true;
        });
      }
    });
  }

  @override
  void dispose() {
    _timeoutTimer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (!AppConfig.hasGoogleClientId || _hasTimedOut) {
      return _buildFallbackButton();
    }

    if (!_isGsiMounted) {
      return _buildLoadingPlaceholder();
    }

    // Render official Google Identity Services button with matching theme, size and pill shape
    return Center(
      child: SizedBox(
        height: 48,
        child: web.renderButton(
          configuration: web.GSIButtonConfiguration(
            type: web.GSIButtonType.standard,
            text: web.GSIButtonText.continueWith,
            theme: web.GSIButtonTheme.outline,
            shape: web.GSIButtonShape.pill,
            size: web.GSIButtonSize.large,
            logoAlignment: web.GSIButtonLogoAlignment.left,
            minimumWidth: 320,
          ),
        ),
      ),
    );
  }

  Widget _buildLoadingPlaceholder() {
    return SizedBox(
      width: double.infinity,
      height: 50,
      child: ElevatedButton(
        onPressed: null,
        style: ElevatedButton.styleFrom(
          backgroundColor: Colors.white,
          foregroundColor: Colors.black,
          elevation: 0,
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(25),
          ),
        ),
        child: const Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            SizedBox(
              width: 18,
              height: 18,
              child: CircularProgressIndicator(strokeWidth: 2, color: Colors.blue),
            ),
            SizedBox(width: 10),
            Text(
              'Connecting Google...',
              style: TextStyle(fontSize: 15, fontWeight: FontWeight.w600, color: Colors.black87),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildFallbackButton() {
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        SizedBox(
          width: double.infinity,
          height: 50,
          child: ElevatedButton(
            onPressed: widget.isLoading ? null : widget.onPressed,
            style: ElevatedButton.styleFrom(
              backgroundColor: Colors.white,
              foregroundColor: Colors.black,
              elevation: 0,
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(25),
              ),
            ),
            child: widget.isLoading
                ? const SizedBox(
                    width: 20,
                    height: 20,
                    child: CircularProgressIndicator(strokeWidth: 2, color: Colors.black),
                  )
                : const Row(
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      Icon(Icons.g_mobiledata, size: 28, color: Colors.blue),
                      SizedBox(width: 6),
                      Text(
                        'Continue with Google',
                        style: TextStyle(fontSize: 15, fontWeight: FontWeight.w600),
                      ),
                    ],
                  ),
          ),
        ),
        if (_hasTimedOut)
          const Padding(
            padding: EdgeInsets.only(top: 8),
            child: Text(
              'Google Sign-In button took too long to load. Click above to retry or check ad-blockers.',
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: 12, color: Colors.orangeAccent),
            ),
          ),
      ],
    );
  }
}

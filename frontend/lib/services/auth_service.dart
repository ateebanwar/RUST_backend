import 'dart:async';
import 'package:flutter/foundation.dart';
import 'package:google_sign_in/google_sign_in.dart';
import '../config/app_config.dart';
import '../models/auth_user.dart';

/// Authentication service for Google Sign-In and Google Drive authorization.
/// 
/// Strictly adheres to security rules:
/// - In-memory token management (no secrets stored in localStorage or persistent storage).
/// - Initiates genuine Google OAuth / OIDC flow for Web (Edge) and mobile.
/// - Separates identity authentication (`idToken`) from Google Drive scope authorization (`accessToken`).
/// - Isolates developer/offline simulated sessions so they cannot masquerade as real Google users.
/// - Emits user changes to listeners.
class AuthService extends ChangeNotifier {
  AuthUser? _currentUser;
  GoogleSignInAccount? _googleAccount;
  bool _isLoading = false;
  String? _lastError;
  bool _isInitialized = false;

  AuthUser? get currentUser => _currentUser;
  bool get isAuthenticated => _currentUser != null;
  bool get isLoading => _isLoading;
  String? get lastError => _lastError;
  bool get isMockSession => _currentUser?.isMock ?? false;

  /// Ensure GoogleSignIn plugin is initialized with current Web Client ID.
  Future<void> ensureInitialized() async {
    if (_isInitialized) return;
    try {
      final clientId = AppConfig.googleClientId.trim();
      await GoogleSignIn.instance.initialize(
        clientId: clientId.isNotEmpty ? clientId : null,
        serverClientId: clientId.isNotEmpty ? clientId : null,
      );
      _isInitialized = true;
    } catch (e) {
      if (kDebugMode) {
        print('GoogleSignIn initialize notice: $e');
      }
    }
  }

  /// Initiate genuine Google Sign-In.
  /// 
  /// In browser (Edge/Web), opens Google's OAuth 2.0 account chooser / consent popup.
  /// Extracts genuine Google OIDC ID token for backend validation (`Authorization: Bearer <idToken>`).
  /// Drive authorization is decoupled and requested via [authorizeDrive] to conform to OAuth 2.0 best practices.
  /// 
  /// Returns `true` if genuine authentication succeeds, `false` otherwise.
  Future<bool> signInWithGoogle() async {
    _isLoading = true;
    _lastError = null;
    notifyListeners();

    try {
      // 1. Verify that a Web Client ID is configured
      if (!AppConfig.hasGoogleClientId) {
        _lastError = 'Google OAuth Web Client ID is not configured. '
            'Please pass --dart-define=GOOGLE_CLIENT_ID=<id> or enter your Web Client ID in Token Settings.';
        _isLoading = false;
        notifyListeners();
        return false;
      }

      // 2. Initialize GoogleSignIn instance
      await ensureInitialized();

      // 3. Initiate interactive Google Sign-In
      final GoogleSignInAccount account = await GoogleSignIn.instance.authenticate();

      // 4. Extract genuine Google OIDC ID token
      final GoogleSignInAuthentication auth = account.authentication;
      final String? idToken = auth.idToken;

      if (idToken == null || idToken.trim().isEmpty) {
        _lastError = 'Google Sign-In succeeded, but no ID token was returned. '
            'Verify that GOOGLE_CLIENT_ID is a Web application client ID in Google Cloud Console '
            'and that your browser origin is added to Authorized JavaScript origins.';
        _isLoading = false;
        notifyListeners();
        return false;
      }

      _googleAccount = account;
      _currentUser = AuthUser(
        id: 'google:${account.id}',
        name: account.displayName ?? (account.email.isNotEmpty ? account.email.split('@').first : 'Google User'),
        email: account.email,
        photoUrl: account.photoUrl,
        idToken: idToken.trim(),
        driveAccessToken: null, // Drive authorization is requested explicitly
        isMock: false,
      );

      _isLoading = false;
      notifyListeners();

      // Attempt immediate Drive authorization for seamless experience if client supports it
      await authorizeDrive(silent: true);

      return true;
    } catch (e) {
      final msg = e.toString().toLowerCase();
      if (msg.contains('popup_closed') || msg.contains('cancel') || msg.contains('abort')) {
        _lastError = null; // Clean cancel, do not treat as fatal error
      } else {
        _lastError = 'Google Sign-In failed: $e';
      }
      _isLoading = false;
      notifyListeners();
      return false;
    }
  }

  /// Request Google Drive AppData scope authorization (`https://www.googleapis.com/auth/drive.appdata`).
  /// 
  /// Uses Google Identity Services scope authorization to obtain an access token
  /// strictly for Google Drive persistence (`X-Drive-Token: <token>`).
  Future<bool> authorizeDrive({bool silent = false}) async {
    if (_currentUser == null) return false;

    // For mock test sessions, Drive authorization is simulated
    if (_currentUser!.isMock) {
      if (_currentUser!.driveAccessToken == null) {
        _currentUser = _currentUser!.copyWith(
          driveAccessToken: 'mock_drive_token_${DateTime.now().millisecondsSinceEpoch}',
        );
        notifyListeners();
      }
      return true;
    }

    final account = _googleAccount;
    if (account == null) {
      if (!silent) {
        _lastError = 'No active Google account found to authorize Drive access.';
        notifyListeners();
      }
      return false;
    }

    try {
      final client = account.authorizationClient;
      final scopes = [AppConfig.googleDriveScope];

      // Check if user has already granted Drive scope
      final existingAuth = await client.authorizationForScopes(scopes);
      String? accessToken = existingAuth?.accessToken;

      // If not granted yet, request Drive scope interactively
      if (accessToken == null || accessToken.trim().isEmpty) {
        if (silent) {
          // If silent attempt fails, do not pop up unexpected prompt
          return false;
        }
        final newAuth = await client.authorizeScopes(scopes);
        accessToken = newAuth.accessToken;
      }

      if (accessToken.trim().isNotEmpty) {
        _currentUser = _currentUser!.copyWith(driveAccessToken: accessToken.trim());
        _lastError = null;
        notifyListeners();
        return true;
      } else {
        if (!silent) {
          _lastError = 'Google Drive authorization was denied. Conversations cannot be synced to Drive.';
          notifyListeners();
        }
        return false;
      }
    } catch (e) {
      if (!silent) {
        _lastError = 'Failed to obtain Google Drive authorization: $e';
        notifyListeners();
      }
      return false;
    }
  }

  /// Launch developer test session for automated UI verification and offline testing.
  /// 
  /// Strictly marked with [AuthUser.isMock] = true so it is never treated as a real Google account.
  Future<bool> launchDevSession({
    String? simulatedName,
    String? simulatedEmail,
    String? customIdToken,
    String? customDriveToken,
  }) async {
    _isLoading = true;
    _lastError = null;
    notifyListeners();

    final name = simulatedName ?? 'Ateeb (Test)';
    final email = simulatedEmail ?? 'ateeb.developer@gmail.com';

    _currentUser = AuthUser(
      id: 'mock_user_${DateTime.now().millisecondsSinceEpoch}',
      name: name,
      email: email,
      photoUrl: null,
      idToken: customIdToken ?? 'mock_dev_id_token_${DateTime.now().millisecondsSinceEpoch}',
      driveAccessToken: customDriveToken ?? 'mock_dev_drive_token_${DateTime.now().millisecondsSinceEpoch}',
      isMock: true,
    );

    _isLoading = false;
    notifyListeners();
    return true;
  }

  /// Sign in using explicitly provided tokens (e.g. backend PAT / API key or manual Google tokens).
  Future<bool> signInWithCustomTokens({
    required String idToken,
    String? driveToken,
    String? name,
    String? email,
  }) async {
    _isLoading = true;
    _lastError = null;
    notifyListeners();

    final cleanIdToken = idToken.trim();
    if (cleanIdToken.isEmpty) {
      _lastError = 'Token cannot be empty';
      _isLoading = false;
      notifyListeners();
      return false;
    }

    _currentUser = AuthUser(
      id: 'custom_user_${DateTime.now().millisecondsSinceEpoch}',
      name: name ?? 'Developer User',
      email: email ?? 'developer@localhost',
      photoUrl: null,
      idToken: cleanIdToken,
      driveAccessToken: driveToken?.trim().isNotEmpty == true ? driveToken!.trim() : null,
      isMock: false,
    );

    _isLoading = false;
    notifyListeners();
    return true;
  }

  /// Update the Google Drive access token (e.g. after refresh or re-authorization).
  void updateDriveToken(String driveToken) {
    if (_currentUser != null) {
      _currentUser = _currentUser!.copyWith(driveAccessToken: driveToken.trim());
      notifyListeners();
    }
  }

  /// Sign out current account and clear all in-memory credentials.
  /// Remote Drive data is never deleted.
  Future<void> signOut() async {
    try {
      if (_googleAccount != null) {
        await GoogleSignIn.instance.signOut();
      }
    } catch (_) {}

    _currentUser = null;
    _googleAccount = null;
    _lastError = null;
    notifyListeners();
  }

  /// Switch account: cleans current session state to allow logging in as another Google account.
  Future<void> switchAccount() async {
    await signOut();
  }
}

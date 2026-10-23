import 'dart:async';
import 'package:file_picker/file_picker.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:http/http.dart' as http;
import '../models/auth_user.dart';
import '../models/chat_event.dart';
import '../models/conversation.dart';
import '../models/drive_status.dart';
import '../models/model_info.dart';
import '../services/auth_service.dart';
import '../services/chat_service.dart';
import '../services/drive_service.dart';

/// Pending attachment selected in the composer before sending.
class PendingAttachment {
  final String name;
  final int size;
  final String? extension;
  final List<int>? bytes;

  PendingAttachment({
    required this.name,
    required this.size,
    this.extension,
    this.bytes,
  });

  String get formattedSize {
    if (size < 1024) return '$size B';
    if (size < 1024 * 1024) return '${(size / 1024).toStringAsFixed(1)} KB';
    return '${(size / (1024 * 1024)).toStringAsFixed(1)} MB';
  }
}

/// Central application state coordinator.
/// 
/// Manages conversation lifecycle, streaming SSE parsing, Drive sync status,
/// composer attachments, and ChatGPT-style mobile drawer navigation.
class AppState extends ChangeNotifier {
  final AuthService authService;
  final ChatService chatService;
  final DriveService driveService;

  // Conversations & History
  List<SyncConversation> _recentConversations = [];
  SyncConversation? _currentConversation;
  List<SyncMessage> _messages = [];
  bool _isLoadingHistory = false;
  String? _historyError;

  // Active Streaming state
  bool _isStreaming = false;
  http.Client? _activeStreamClient;
  StreamSubscription<ChatSseEvent>? _streamSubscription;

  // Reasoning / Think mode
  ThinkMode _selectedThinkMode = ThinkMode.auto;

  // Google Drive status
  SyncStatus? _driveSyncStatus;
  bool _isSyncingDrive = false;
  String? _driveSyncError;

  // Pending attachments in composer
  final List<PendingAttachment> _pendingAttachments = [];

  // Theme & Preferences (In-memory preferences per architectural guidelines)
  ThemeMode _themeMode = ThemeMode.dark;
  String _selectedLanguage = 'English (US)';

  // Models catalog
  List<ModelInfo> _availableModels = [];

  // Getters
  List<SyncConversation> get recentConversations => _recentConversations;
  SyncConversation? get currentConversation => _currentConversation;
  List<SyncMessage> get messages => _messages;
  bool get isLoadingHistory => _isLoadingHistory;
  String? get historyError => _historyError;
  bool get isStreaming => _isStreaming;
  ThinkMode get selectedThinkMode => _selectedThinkMode;
  SyncStatus? get driveSyncStatus => _driveSyncStatus;
  bool get isSyncingDrive => _isSyncingDrive;
  String? get driveSyncError => _driveSyncError;
  List<PendingAttachment> get pendingAttachments => _pendingAttachments;
  ThemeMode get themeMode => _themeMode;
  String get selectedLanguage => _selectedLanguage;
  List<ModelInfo> get availableModels => _availableModels;
  AuthUser? get currentUser => authService.currentUser;
  bool get isAuthenticated => authService.isAuthenticated;

  AppState({
    required this.authService,
    required this.chatService,
    required this.driveService,
  }) {
    authService.addListener(_onAuthChanged);
  }

  void _onAuthChanged() {
    if (authService.isAuthenticated) {
      loadInitialData();
    } else {
      _recentConversations = [];
      _currentConversation = null;
      _messages = [];
      _pendingAttachments.clear();
      _driveSyncStatus = null;
      notifyListeners();
    }
  }

  /// Initial hydration after authentication
  Future<void> loadInitialData() async {
    final user = authService.currentUser;
    if (user == null) return;

    await Future.wait([
      loadConversations(),
      checkDriveStatus(),
      loadModels(),
    ]);
  }

  /// Load conversations from Google Drive via backend
  Future<void> loadConversations() async {
    final user = authService.currentUser;
    if (user == null) return;

    _isLoadingHistory = true;
    _historyError = null;
    notifyListeners();

    try {
      _recentConversations = await driveService.listConversations(user);
      _recentConversations.sort((a, b) => b.updatedAt.compareTo(a.updatedAt));
      if (kDebugMode) {
        print('[RUNTIME-TRACE][FRONTEND] loadConversations: Successfully loaded ${_recentConversations.length} conversations into sidebar: ${_recentConversations.map((c) => c.title).toList()}');
      }
    } catch (e) {
      _historyError = 'Could not load conversation history from Drive: $e';
    } finally {
      _isLoadingHistory = false;
      notifyListeners();
    }
  }

  /// Query backend for Drive synchronization status
  Future<void> checkDriveStatus() async {
    final user = authService.currentUser;
    if (user == null) return;

    try {
      _driveSyncStatus = await driveService.getSyncStatus(user);
      _driveSyncError = null;
      notifyListeners();
    } catch (e) {
      _driveSyncError = e.toString();
      notifyListeners();
    }
  }

  /// Request Google Drive AppData scope authorization interactively
  Future<bool> authorizeDrive() async {
    final success = await authService.authorizeDrive();
    if (success) {
      _driveSyncError = null;
      final currentUser = authService.currentUser;
      if (currentUser != null &&
          currentUser.hasDriveToken &&
          _currentConversation != null &&
          _messages.isNotEmpty) {
        try {
          final convToSave = _currentConversation!.copyWith(
            ownerSubject: currentUser.id,
            messages: _messages.map((m) => m.copyWith(isSaved: true, saveError: null)).toList(),
          );
          final saved = await driveService.saveConversation(currentUser, convToSave);
          _currentConversation = saved;
          _messages = _messages.map((m) => m.copyWith(isSaved: true, saveError: null)).toList();
          notifyListeners();
        } catch (e) {
          _driveSyncError = 'Connected to Drive, but could not save active conversation: $e';
          notifyListeners();
        }
      }
      await checkDriveStatus();
      await loadConversations();
    }
    return success;
  }

  /// Trigger manual Drive synchronization
  Future<void> triggerDriveSync() async {
    final user = authService.currentUser;
    if (user == null || _isSyncingDrive) return;

    if (!user.hasDriveToken) {
      final authorized = await authorizeDrive();
      if (!authorized) return;
    }

    _isSyncingDrive = true;
    notifyListeners();

    try {
      final currentUser = authService.currentUser ?? user;
      if (currentUser.hasDriveToken && _currentConversation != null && _messages.isNotEmpty) {
        try {
          final convToSave = _currentConversation!.copyWith(
            ownerSubject: currentUser.id,
            messages: _messages.map((m) => m.copyWith(isSaved: true, saveError: null)).toList(),
          );
          final saved = await driveService.saveConversation(currentUser, convToSave);
          _currentConversation = saved;
          _messages = _messages.map((m) => m.copyWith(isSaved: true, saveError: null)).toList();
        } catch (_) {
          // Best-effort pre-sync active conversation persistence
        }
      }
      await driveService.triggerSync(currentUser);
      await checkDriveStatus();
      await loadConversations();
    } catch (e) {
      _driveSyncError = 'Sync failed: $e';
    } finally {
      _isSyncingDrive = false;
      notifyListeners();
    }
  }

  /// Fetch models catalog
  Future<void> loadModels() async {
    final user = authService.currentUser;
    if (user == null) return;

    try {
      _availableModels = await chatService.getModels(user);
      notifyListeners();
    } catch (_) {
      // Backend may be offline or models route unconfigured
    }
  }

  /// Start a genuinely new conversation
  void startNewChat() {
    if (_isStreaming) {
      stopStreaming();
    }

    final user = authService.currentUser;
    final owner = user?.id ?? 'guest';
    final newConvId = 'conv_${DateTime.now().millisecondsSinceEpoch}';

    _currentConversation = SyncConversation.newConversation(
      id: newConvId,
      ownerSubject: owner,
      title: 'New Chat',
    );
    _messages = [];
    _pendingAttachments.clear();
    notifyListeners();
  }

  /// Open an existing saved conversation
  Future<void> selectConversation(String conversationId) async {
    if (_isStreaming) {
      stopStreaming();
    }

    final user = authService.currentUser;
    if (user == null) return;

    _isLoadingHistory = true;
    notifyListeners();

    try {
      final conv = await driveService.getConversation(user, conversationId);
      _currentConversation = conv;
      _messages = List.from(conv.messages);
      _pendingAttachments.clear();
    } catch (e) {
      _historyError = 'Failed to load conversation: $e';
    } finally {
      _isLoadingHistory = false;
      notifyListeners();
    }
  }

  /// Rename conversation
  Future<void> renameConversation(String conversationId, String newTitle) async {
    final user = authService.currentUser;
    if (user == null) return;

    final idx = _recentConversations.indexWhere((c) => c.id == conversationId);
    if (idx != -1) {
      final updated = _recentConversations[idx].copyWith(title: newTitle);
      try {
        await driveService.saveConversation(user, updated);
        _recentConversations[idx] = updated;
        if (_currentConversation?.id == conversationId) {
          _currentConversation = updated;
        }
        notifyListeners();
      } catch (e) {
        _historyError = 'Rename failed: $e';
        notifyListeners();
      }
    }
  }

  /// Delete conversation with soft-delete on Google Drive
  Future<void> deleteConversation(String conversationId) async {
    final user = authService.currentUser;
    if (user == null) return;

    try {
      await driveService.deleteConversation(user, conversationId);
      _recentConversations.removeWhere((c) => c.id == conversationId);
      if (_currentConversation?.id == conversationId) {
        startNewChat();
      } else {
        notifyListeners();
      }
    } catch (e) {
      _historyError = 'Delete failed: $e';
      notifyListeners();
    }
  }

  /// Submit user prompt and begin streaming assistant response
  Future<void> sendMessage(String text) async {
    final user = authService.currentUser;
    if (user == null || _isStreaming || text.trim().isEmpty) return;

    // Ensure we have an active conversation ID
    if (_currentConversation == null) {
      startNewChat();
    }

    final convId = _currentConversation!.id;
    final nowIso = DateTime.now().toUtc().toIso8601String();
    final userMsgId = 'msg_${DateTime.now().millisecondsSinceEpoch}_usr';

    // 1. Add user message
    final userMessage = SyncMessage(
      id: userMsgId,
      conversationId: convId,
      role: 'user',
      content: text.trim(),
      timestamp: nowIso,
      isSaved: user.hasDriveToken,
    );

    _messages.add(userMessage);

    // If conversation title is still default, update it with first prompt
    if (_currentConversation!.title == 'New Chat' || _currentConversation!.title.isEmpty) {
      final cleanTitle = text.trim().length > 30
          ? '${text.trim().substring(0, 30)}...'
          : text.trim();
      _currentConversation = _currentConversation!.copyWith(title: cleanTitle);
    }

    // 2. Add placeholder assistant response
    final asstMsgId = 'msg_${DateTime.now().millisecondsSinceEpoch + 1}_ast';
    final assistantMessage = SyncMessage(
      id: asstMsgId,
      conversationId: convId,
      role: 'assistant',
      content: '',
      timestamp: DateTime.now().toUtc().toIso8601String(),
      isStreaming: true,
      isSaved: user.hasDriveToken, // Optimistically true only if Drive token is present
    );

    _messages.add(assistantMessage);
    _isStreaming = true;
    _pendingAttachments.clear();
    notifyListeners();

    // 3. Initiate SSE Streaming
    _activeStreamClient = http.Client();
    final stream = chatService.streamChat(
      user: user,
      conversationId: convId,
      messages: List.from(_messages.where((m) => !m.isStreaming)),
      routingMode: _selectedThinkMode.routingMode,
      dedicatedClient: _activeStreamClient,
    );

    String accumulated = '';
    String? usedModel;
    String? usedProvider;

    _streamSubscription = stream.listen(
      (event) {
        final asstIndex = _messages.indexWhere((m) => m.id == asstMsgId);
        if (asstIndex == -1) return;

        if (event is ChunkEvent) {
          accumulated += event.delta;
          _messages[asstIndex] = _messages[asstIndex].copyWith(
            content: accumulated,
            isStreaming: true,
          );
          notifyListeners();
        } else if (event is RoutingEvent) {
          usedModel = event.modelUsed;
          usedProvider = event.providerUsed;
          _messages[asstIndex] = _messages[asstIndex].copyWith(
            modelId: usedModel,
            providerId: usedProvider,
          );
          notifyListeners();
        } else if (event is SaveErrorEvent) {
          // CRITICAL: Handle save_error explicitly
          _messages[asstIndex] = _messages[asstIndex].copyWith(
            isSaved: false,
            saveError: event.message,
            isRetryable: event.retryable,
          );
          notifyListeners();
        } else if (event is DoneEvent) {
          final isSaveFailed = event.isSaveFailed;
          _messages[asstIndex] = _messages[asstIndex].copyWith(
            content: accumulated,
            isStreaming: false,
            isSaved: user.hasDriveToken ? !isSaveFailed : false,
            saveError: isSaveFailed
                ? (event.errorMessage ?? 'Saving to Google Drive failed')
                : null,
            isRetryable: isSaveFailed,
          );
          _isStreaming = false;
          notifyListeners();

          // Refresh recent conversations from Drive if successfully saved
          if (!isSaveFailed && user.hasDriveToken) {
            loadConversations();
            checkDriveStatus();
          }
        } else if (event is ErrorEvent) {
          _messages[asstIndex] = _messages[asstIndex].copyWith(
            content: accumulated.isEmpty
                ? 'Error: ${event.message}'
                : accumulated,
            isStreaming: false,
            isSaved: false,
            saveError: event.message,
            isRetryable: event.retryable,
          );
          _isStreaming = false;
          notifyListeners();
        }
      },
      onError: (err) {
        final asstIndex = _messages.indexWhere((m) => m.id == asstMsgId);
        if (asstIndex != -1) {
          _messages[asstIndex] = _messages[asstIndex].copyWith(
            isStreaming: false,
            isSaved: false,
            saveError: 'Stream interrupted: $err',
            isRetryable: true,
          );
        }
        _isStreaming = false;
        notifyListeners();
      },
      onDone: () {
        _isStreaming = false;
        notifyListeners();
      },
    );
  }

  /// Stop streaming generation
  void stopStreaming() {
    _streamSubscription?.cancel();
    _streamSubscription = null;
    _activeStreamClient?.close();
    _activeStreamClient = null;
    _isStreaming = false;

    // Mark current streaming message as finished
    final idx = _messages.indexWhere((m) => m.isStreaming);
    if (idx != -1) {
      _messages[idx] = _messages[idx].copyWith(isStreaming: false);
    }
    notifyListeners();
  }

  /// Retry last assistant response
  void retryLastMessage() {
    if (_messages.isEmpty || _isStreaming) return;
    final lastMsg = _messages.last;
    if (lastMsg.isAssistant) {
      _messages.removeLast();
      if (_messages.isNotEmpty && _messages.last.isUser) {
        final lastUserPrompt = _messages.removeLast().content;
        sendMessage(lastUserPrompt);
      }
    }
  }

  /// Reasoning / Think selector
  void setThinkMode(ThinkMode mode) {
    _selectedThinkMode = mode;
    notifyListeners();
  }

  /// Theme switcher (Dark / Light / System)
  void setThemeMode(ThemeMode mode) {
    _themeMode = mode;
    notifyListeners();
  }

  /// Language selector
  void setLanguage(String lang) {
    _selectedLanguage = lang;
    notifyListeners();
  }

  /// Add attachment to composer
  Future<void> addAttachment(PlatformFile file) async {
    final size = file.lengthSync() ?? (await file.length()) ?? 0;
    List<int>? bytes;
    try {
      bytes = await file.readAsBytes();
    } catch (_) {}

    _pendingAttachments.add(
      PendingAttachment(
        name: file.name,
        size: size,
        extension: file.extension,
        bytes: bytes,
      ),
    );
    notifyListeners();
  }

  /// Remove attachment from composer
  void removeAttachment(int index) {
    if (index >= 0 && index < _pendingAttachments.length) {
      _pendingAttachments.removeAt(index);
      notifyListeners();
    }
  }

  @override
  void dispose() {
    authService.removeListener(_onAuthChanged);
    _streamSubscription?.cancel();
    _activeStreamClient?.close();
    super.dispose();
  }
}

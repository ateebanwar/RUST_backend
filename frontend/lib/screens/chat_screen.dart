import 'package:flutter/material.dart';
import '../services/speech_service.dart';
import '../state/app_state.dart';
import '../theme/app_theme.dart';
import '../widgets/app_drawer.dart';
import '../widgets/chat_app_bar.dart';
import '../widgets/composer.dart';
import '../widgets/message_bubble.dart';
import 'documents_workspace_screen.dart';
import 'images_workspace_screen.dart';
import 'personalization_screen.dart';
import 'search_screen.dart';

/// Main ChatGPT Android-inspired Chat Screen.
class ChatScreen extends StatefulWidget {
  final AppState appState;
  final SpeechService speechService;

  const ChatScreen({
    super.key,
    required this.appState,
    required this.speechService,
  });

  @override
  State<ChatScreen> createState() => _ChatScreenState();
}

class _ChatScreenState extends State<ChatScreen> {
  final GlobalKey<ScaffoldState> _scaffoldKey = GlobalKey<ScaffoldState>();
  final ScrollController _scrollController = ScrollController();
  bool _userScrolledUp = false;

  @override
  void initState() {
    super.initState();
    _scrollController.addListener(_onScroll);
    widget.appState.addListener(_onAppStateChanged);
  }

  void _onScroll() {
    if (!_scrollController.hasClients) return;
    final maxScroll = _scrollController.position.maxScrollExtent;
    final currentScroll = _scrollController.position.pixels;
    // If user is more than 100px from the bottom, mark as scrolled up
    _userScrolledUp = (maxScroll - currentScroll) > 100;
  }

  void _onAppStateChanged() {
    // Autoscroll to bottom during streaming ONLY if user hasn't scrolled up
    if (!_userScrolledUp && _scrollController.hasClients) {
      WidgetsBinding.instance.addPostFrameCallback((_) {
        if (_scrollController.hasClients) {
          _scrollController.animateTo(
            _scrollController.position.maxScrollExtent,
            duration: const Duration(milliseconds: 150),
            curve: Curves.easeOut,
          );
        }
      });
    }
  }

  @override
  void dispose() {
    _scrollController.removeListener(_onScroll);
    widget.appState.removeListener(_onAppStateChanged);
    _scrollController.dispose();
    super.dispose();
  }

  void _openSearch() {
    Navigator.push(
      context,
      MaterialPageRoute(
        builder: (_) => SearchScreen(
          appState: widget.appState,
          onConversationSelected: () {
            // Screen handles pop
          },
        ),
      ),
    );
  }

  void _openImages() {
    Navigator.push(
      context,
      MaterialPageRoute(
        builder: (_) => const ImagesWorkspaceScreen(),
      ),
    );
  }

  void _openDocuments() {
    Navigator.push(
      context,
      MaterialPageRoute(
        builder: (_) => DocumentsWorkspaceScreen(appState: widget.appState),
      ),
    );
  }

  void _openPersonalization() {
    Navigator.push(
      context,
      MaterialPageRoute(
        builder: (_) => PersonalizationScreen(appState: widget.appState),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final currentConv = widget.appState.currentConversation;
    final messages = widget.appState.messages;
    final syncStatus = widget.appState.driveSyncStatus;
    final isSyncing = widget.appState.isSyncingDrive;
    final syncError = widget.appState.driveSyncError;
    final user = widget.appState.currentUser;

    return Scaffold(
      key: _scaffoldKey,
      backgroundColor: AppTheme.darkBg,
      // Sidebar drawer collapsed by default
      drawer: AppDrawer(
        appState: widget.appState,
        onOpenSearch: _openSearch,
        onOpenImages: _openImages,
        onOpenDocuments: _openDocuments,
        onOpenPersonalization: _openPersonalization,
        onSwitchAccount: () => widget.appState.authService.switchAccount(),
        onSignOut: () => widget.appState.authService.signOut(),
      ),
      appBar: ChatAppBar(
        title: currentConv?.title.isNotEmpty == true && currentConv!.title != 'New Chat'
            ? (currentConv.title.length > 20
                ? '${currentConv.title.substring(0, 20)}...'
                : currentConv.title)
            : 'AI Assistant',
        syncStatus: syncStatus,
        isSyncing: isSyncing,
        hasDriveToken: widget.appState.currentUser?.hasDriveToken ?? false,
        syncError: syncError,
        onMenuTap: () => _scaffoldKey.currentState?.openDrawer(),
        onNewChatTap: () => widget.appState.startNewChat(),
        onSyncTap: () => widget.appState.triggerDriveSync(),
      ),
      body: SafeArea(
        child: Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 860),
            child: Column(
              children: [
                // Conversation Area
                Expanded(
                  child: messages.isEmpty
                      ? _buildEmptyState()
                      : ListView.builder(
                          controller: _scrollController,
                          padding: const EdgeInsets.only(top: 12, bottom: 20),
                          itemCount: messages.length,
                          itemBuilder: (context, index) {
                            final msg = messages[index];
                            return MessageBubble(
                              message: msg,
                              onRetry: () => widget.appState.retryLastMessage(),
                            );
                          },
                        ),
                ),

                // Subtle Drive authorization prompt if user has not yet authorized Drive
                if (user != null && !user.hasDriveToken && !user.isMock)
                  Container(
                    margin: const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
                    padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
                    decoration: BoxDecoration(
                      color: AppTheme.darkSurfaceVariant,
                      borderRadius: BorderRadius.circular(10),
                      border: Border.all(color: AppTheme.accentGreen.withOpacity(0.3)),
                    ),
                    child: Row(
                      children: [
                        const Icon(Icons.cloud_upload_outlined, size: 16, color: AppTheme.accentGreen),
                        const SizedBox(width: 8),
                        const Expanded(
                          child: Text(
                            'Authorize Google Drive to sync conversations.',
                            style: TextStyle(fontSize: 12, color: AppTheme.textSecondaryDark),
                          ),
                        ),
                        TextButton(
                          onPressed: () => widget.appState.authorizeDrive(),
                          style: TextButton.styleFrom(
                            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
                            minimumSize: Size.zero,
                          ),
                          child: const Text('Connect', style: TextStyle(fontSize: 12, color: AppTheme.accentGreen, fontWeight: FontWeight.bold)),
                        ),
                      ],
                    ),
                  ),

                // Drive sync error banner if sync or conversation persistence fails
                if (syncError != null && user != null && user.hasDriveToken)
                  Container(
                    margin: const EdgeInsets.symmetric(horizontal: 16, vertical: 6),
                    padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
                    decoration: BoxDecoration(
                      color: AppTheme.statusError.withOpacity(0.12),
                      borderRadius: BorderRadius.circular(10),
                      border: Border.all(color: AppTheme.statusError.withOpacity(0.3)),
                    ),
                    child: Row(
                      children: [
                        const Icon(Icons.error_outline, size: 16, color: AppTheme.statusError),
                        const SizedBox(width: 8),
                        Expanded(
                          child: Text(
                            syncError,
                            style: const TextStyle(fontSize: 12, color: AppTheme.statusError),
                            maxLines: 2,
                            overflow: TextOverflow.ellipsis,
                          ),
                        ),
                        TextButton(
                          onPressed: () => widget.appState.triggerDriveSync(),
                          style: TextButton.styleFrom(
                            padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
                            minimumSize: Size.zero,
                          ),
                          child: const Text('Retry', style: TextStyle(fontSize: 12, color: AppTheme.statusError, fontWeight: FontWeight.bold)),
                        ),
                      ],
                    ),
                  ),

                // Composer at the bottom
                MessageComposer(
                  appState: widget.appState,
                  speechService: widget.speechService,
                  onDriveFilesTap: () {
                    ScaffoldMessenger.of(context).showSnackBar(
                      const SnackBar(
                        content: Text('Drive files can be managed in Google Drive or Documents workspace'),
                        backgroundColor: AppTheme.darkSurfaceVariant,
                      ),
                    );
                  },
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }

  Widget _buildEmptyState() {
    final suggestions = [
      'Help me write a Rust function with error handling',
      'Explain how OAuth 2.0 PKCE works simply',
      'Analyze the pros and cons of serverless vs containers',
      'Write a clean, responsive mobile UI component',
    ];

    return Center(
      child: SingleChildScrollView(
        padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 20),
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            Container(
              width: 52,
              height: 52,
              decoration: BoxDecoration(
                color: AppTheme.darkSurfaceVariant,
                shape: BoxShape.circle,
                border: Border.all(color: AppTheme.darkBorder, width: 1),
              ),
              child: const Icon(
                Icons.auto_awesome,
                color: Colors.white,
                size: 26,
              ),
            ),
            const SizedBox(height: 16),
            const Text(
              'What can I help with today?',
              style: TextStyle(
                fontSize: 20,
                fontWeight: FontWeight.w600,
                color: AppTheme.textPrimaryDark,
                letterSpacing: -0.3,
              ),
            ),
            const SizedBox(height: 28),
            // Prompt suggestion chips
            Wrap(
              spacing: 8,
              runSpacing: 8,
              alignment: WrapAlignment.center,
              children: suggestions.map((text) {
                return InkWell(
                  onTap: () => widget.appState.sendMessage(text),
                  borderRadius: BorderRadius.circular(16),
                  child: Container(
                    constraints: const BoxConstraints(maxWidth: 320),
                    padding: const EdgeInsets.symmetric(horizontal: 14, vertical: 10),
                    decoration: BoxDecoration(
                      color: AppTheme.darkSurfaceVariant,
                      borderRadius: BorderRadius.circular(16),
                      border: Border.all(color: AppTheme.darkBorder, width: 1),
                    ),
                    child: Row(
                      mainAxisSize: MainAxisSize.min,
                      children: [
                        const Icon(Icons.arrow_outward, size: 14, color: AppTheme.textSecondaryDark),
                        const SizedBox(width: 8),
                        Flexible(
                          child: Text(
                            text,
                            style: const TextStyle(
                              fontSize: 13,
                              color: AppTheme.textPrimaryDark,
                            ),
                          ),
                        ),
                      ],
                    ),
                  ),
                );
              }).toList(),
            ),
          ],
        ),
      ),
    );
  }
}

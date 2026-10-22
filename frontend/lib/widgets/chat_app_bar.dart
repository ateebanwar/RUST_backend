import 'package:flutter/material.dart';
import '../models/drive_status.dart';
import '../theme/app_theme.dart';
import 'sync_status_badge.dart';

/// App bar for the ChatGPT mobile interface.
class ChatAppBar extends StatelessWidget implements PreferredSizeWidget {
  final String title;
  final SyncStatus? syncStatus;
  final bool isSyncing;
  final bool hasDriveToken;
  final String? syncError;
  final VoidCallback onMenuTap;
  final VoidCallback onNewChatTap;
  final VoidCallback onSyncTap;

  const ChatAppBar({
    super.key,
    this.title = 'AI Assistant',
    this.syncStatus,
    this.isSyncing = false,
    this.hasDriveToken = true,
    this.syncError,
    required this.onMenuTap,
    required this.onNewChatTap,
    required this.onSyncTap,
  });

  @override
  Size get preferredSize => const Size.fromHeight(kToolbarHeight);

  @override
  Widget build(BuildContext context) {
    return AppBar(
      leading: IconButton(
        icon: const Icon(Icons.menu, size: 22, color: AppTheme.textPrimaryDark),
        onPressed: onMenuTap,
        tooltip: 'Open sidebar',
      ),
      title: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            title,
            style: const TextStyle(
              fontSize: 16,
              fontWeight: FontWeight.w600,
              color: AppTheme.textPrimaryDark,
            ),
          ),
          const SizedBox(width: 8),
          SyncStatusBadge(
            status: syncStatus,
            isSyncing: isSyncing,
            hasDriveToken: hasDriveToken,
            errorMessage: syncError,
            onSyncTap: onSyncTap,
          ),
        ],
      ),
      actions: [
        IconButton(
          icon: const Icon(Icons.edit_note, size: 22, color: AppTheme.textPrimaryDark),
          onPressed: onNewChatTap,
          tooltip: 'New Chat',
        ),
      ],
    );
  }
}

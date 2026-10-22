import 'package:flutter/material.dart';
import '../models/conversation.dart';
import '../state/app_state.dart';
import '../theme/app_theme.dart';
import 'account_popup.dart';

/// ChatGPT Android-inspired navigation drawer.
/// 
/// Contains:
/// 1. New Chat
/// 2. Search (opens dedicated search screen)
/// 3. Images (opens image workspace)
/// 4. Documents (opens document workspace)
/// 5. Recent Chats (collapsible section with time grouping, rename, delete)
/// 6. Pinned Account section at the bottom.
class AppDrawer extends StatefulWidget {
  final AppState appState;
  final VoidCallback onOpenSearch;
  final VoidCallback onOpenImages;
  final VoidCallback onOpenDocuments;
  final VoidCallback onOpenPersonalization;
  final VoidCallback onSwitchAccount;
  final VoidCallback onSignOut;

  const AppDrawer({
    super.key,
    required this.appState,
    required this.onOpenSearch,
    required this.onOpenImages,
    required this.onOpenDocuments,
    required this.onOpenPersonalization,
    required this.onSwitchAccount,
    required this.onSignOut,
  });

  @override
  State<AppDrawer> createState() => _AppDrawerState();
}

class _AppDrawerState extends State<AppDrawer> {
  bool _isRecentChatsExpanded = true;

  Map<String, List<SyncConversation>> _groupConversations(List<SyncConversation> convs) {
    final now = DateTime.now();
    final today = DateTime(now.year, now.month, now.day);
    final yesterday = today.subtract(const Duration(days: 1));
    final sevenDaysAgo = today.subtract(const Duration(days: 7));

    final Map<String, List<SyncConversation>> groups = {
      'Today': [],
      'Yesterday': [],
      'Previous 7 Days': [],
      'Older': [],
    };

    for (final c in convs) {
      DateTime? dt;
      try {
        dt = DateTime.parse(c.updatedAt).toLocal();
      } catch (_) {
        dt = null;
      }

      if (dt == null) {
        groups['Older']!.add(c);
      } else if (dt.isAfter(today)) {
        groups['Today']!.add(c);
      } else if (dt.isAfter(yesterday)) {
        groups['Yesterday']!.add(c);
      } else if (dt.isAfter(sevenDaysAgo)) {
        groups['Previous 7 Days']!.add(c);
      } else {
        groups['Older']!.add(c);
      }
    }

    // Remove empty groups
    groups.removeWhere((key, value) => value.isEmpty);
    return groups;
  }

  void _showRenameDialog(SyncConversation conv) {
    final controller = TextEditingController(text: conv.title);
    showDialog(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: AppTheme.darkSurface,
        title: const Text('Rename Chat', style: TextStyle(color: AppTheme.textPrimaryDark, fontSize: 16)),
        content: TextField(
          controller: controller,
          autofocus: true,
          style: const TextStyle(color: AppTheme.textPrimaryDark),
          decoration: const InputDecoration(
            hintText: 'Enter new title',
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx),
            child: const Text('Cancel', style: TextStyle(color: AppTheme.textSecondaryDark)),
          ),
          ElevatedButton(
            onPressed: () {
              final newTitle = controller.text.trim();
              if (newTitle.isNotEmpty) {
                widget.appState.renameConversation(conv.id, newTitle);
              }
              Navigator.pop(ctx);
            },
            style: ElevatedButton.styleFrom(backgroundColor: AppTheme.accentGreen),
            child: const Text('Save', style: TextStyle(color: Colors.white)),
          ),
        ],
      ),
    );
  }

  void _showDeleteDialog(SyncConversation conv) {
    showDialog(
      context: context,
      builder: (ctx) => AlertDialog(
        backgroundColor: AppTheme.darkSurface,
        title: const Text('Delete Chat?', style: TextStyle(color: AppTheme.textPrimaryDark, fontSize: 16)),
        content: Text(
          'Are you sure you want to delete "${conv.title}"? It will be soft-deleted on Google Drive.',
          style: const TextStyle(color: AppTheme.textSecondaryDark, fontSize: 14),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(ctx),
            child: const Text('Cancel', style: TextStyle(color: AppTheme.textSecondaryDark)),
          ),
          ElevatedButton(
            onPressed: () {
              widget.appState.deleteConversation(conv.id);
              Navigator.pop(ctx);
            },
            style: ElevatedButton.styleFrom(backgroundColor: AppTheme.statusError),
            child: const Text('Delete', style: TextStyle(color: Colors.white)),
          ),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final user = widget.appState.currentUser;
    final currentConv = widget.appState.currentConversation;
    final recentConvs = widget.appState.recentConversations;
    final grouped = _groupConversations(recentConvs);

    return Drawer(
      backgroundColor: AppTheme.darkSidebar,
      child: SafeArea(
        child: Column(
          children: [
            // Top Action Items
            Padding(
              padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 8),
              child: Column(
                children: [
                  // New Chat
                  ListTile(
                    shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(10)),
                    leading: const Icon(Icons.edit_note, color: AppTheme.textPrimaryDark, size: 22),
                    title: const Text(
                      'New Chat',
                      style: TextStyle(
                        fontSize: 14,
                        fontWeight: FontWeight.w600,
                        color: AppTheme.textPrimaryDark,
                      ),
                    ),
                    onTap: () {
                      widget.appState.startNewChat();
                      Navigator.pop(context);
                    },
                  ),
                  // Search
                  ListTile(
                    shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(10)),
                    leading: const Icon(Icons.search, color: AppTheme.textPrimaryDark, size: 20),
                    title: const Text(
                      'Search',
                      style: TextStyle(fontSize: 14, color: AppTheme.textPrimaryDark),
                    ),
                    onTap: () {
                      Navigator.pop(context);
                      widget.onOpenSearch();
                    },
                  ),
                  // Images
                  ListTile(
                    shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(10)),
                    leading: const Icon(Icons.image_outlined, color: AppTheme.textPrimaryDark, size: 20),
                    title: const Text(
                      'Images',
                      style: TextStyle(fontSize: 14, color: AppTheme.textPrimaryDark),
                    ),
                    onTap: () {
                      Navigator.pop(context);
                      widget.onOpenImages();
                    },
                  ),
                  // Documents
                  ListTile(
                    shape: RoundedRectangleBorder(borderRadius: BorderRadius.circular(10)),
                    leading: const Icon(Icons.description_outlined, color: AppTheme.textPrimaryDark, size: 20),
                    title: const Text(
                      'Documents',
                      style: TextStyle(fontSize: 14, color: AppTheme.textPrimaryDark),
                    ),
                    onTap: () {
                      Navigator.pop(context);
                      widget.onOpenDocuments();
                    },
                  ),
                ],
              ),
            ),

            const Divider(color: AppTheme.darkDivider, height: 1),

            // Recent Chats section with collapse/expand header
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 12, 12, 4),
              child: InkWell(
                onTap: () {
                  setState(() {
                    _isRecentChatsExpanded = !_isRecentChatsExpanded;
                  });
                },
                borderRadius: BorderRadius.circular(6),
                child: Padding(
                  padding: const EdgeInsets.symmetric(vertical: 4),
                  child: Row(
                    children: [
                      const Text(
                        'Recent Chats',
                        style: TextStyle(
                          fontSize: 12,
                          fontWeight: FontWeight.w600,
                          color: AppTheme.textMutedDark,
                          letterSpacing: 0.3,
                        ),
                      ),
                      const Spacer(),
                      Icon(
                        _isRecentChatsExpanded
                            ? Icons.keyboard_arrow_down
                            : Icons.keyboard_arrow_right,
                        size: 18,
                        color: AppTheme.textMutedDark,
                      ),
                    ],
                  ),
                ),
              ),
            ),

            // Scrollable Recent Chats List
            Expanded(
              child: _isRecentChatsExpanded
                  ? (widget.appState.isLoadingHistory && recentConvs.isEmpty)
                      ? const Center(
                          child: SizedBox(
                            width: 20,
                            height: 20,
                            child: CircularProgressIndicator(strokeWidth: 2, color: AppTheme.textSecondaryDark),
                          ),
                        )
                      : recentConvs.isEmpty
                          ? const Center(
                              child: Text(
                                'No recent chats',
                                style: TextStyle(fontSize: 13, color: AppTheme.textMutedDark),
                              ),
                            )
                          : ListView.builder(
                              padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
                              itemCount: grouped.keys.length,
                              itemBuilder: (context, groupIdx) {
                                final header = grouped.keys.elementAt(groupIdx);
                                final convList = grouped[header]!;

                                return Column(
                                  crossAxisAlignment: CrossAxisAlignment.start,
                                  children: [
                                    Padding(
                                      padding: const EdgeInsets.fromLTRB(8, 10, 8, 4),
                                      child: Text(
                                        header,
                                        style: const TextStyle(
                                          fontSize: 11,
                                          fontWeight: FontWeight.w500,
                                          color: AppTheme.textMutedDark,
                                        ),
                                      ),
                                    ),
                                    ...convList.map((conv) {
                                      final isSelected = currentConv?.id == conv.id;
                                      return Container(
                                        margin: const EdgeInsets.symmetric(vertical: 1),
                                        decoration: BoxDecoration(
                                          color: isSelected
                                              ? AppTheme.darkSurfaceVariant
                                              : Colors.transparent,
                                          borderRadius: BorderRadius.circular(8),
                                        ),
                                        child: ListTile(
                                          dense: true,
                                          visualDensity: VisualDensity.compact,
                                          contentPadding: const EdgeInsets.only(left: 10, right: 4),
                                          title: Text(
                                            conv.title,
                                            maxLines: 1,
                                            overflow: TextOverflow.ellipsis,
                                            style: TextStyle(
                                              fontSize: 13,
                                              color: isSelected
                                                  ? AppTheme.textPrimaryDark
                                                  : AppTheme.textSecondaryDark,
                                              fontWeight: isSelected
                                                  ? FontWeight.w600
                                                  : FontWeight.normal,
                                            ),
                                          ),
                                          trailing: PopupMenuButton<String>(
                                            icon: const Icon(
                                              Icons.more_vert,
                                              size: 16,
                                              color: AppTheme.textMutedDark,
                                            ),
                                            color: AppTheme.darkSurface,
                                            itemBuilder: (ctx) => [
                                              const PopupMenuItem(
                                                value: 'rename',
                                                child: Row(
                                                  children: [
                                                    Icon(Icons.edit, size: 14, color: AppTheme.textPrimaryDark),
                                                    SizedBox(width: 8),
                                                    Text('Rename', style: TextStyle(fontSize: 13, color: AppTheme.textPrimaryDark)),
                                                  ],
                                                ),
                                              ),
                                              const PopupMenuItem(
                                                value: 'delete',
                                                child: Row(
                                                  children: [
                                                    Icon(Icons.delete, size: 14, color: AppTheme.statusError),
                                                    SizedBox(width: 8),
                                                    Text('Delete', style: TextStyle(fontSize: 13, color: AppTheme.statusError)),
                                                  ],
                                                ),
                                              ),
                                            ],
                                            onSelected: (val) {
                                              if (val == 'rename') {
                                                _showRenameDialog(conv);
                                              } else if (val == 'delete') {
                                                _showDeleteDialog(conv);
                                              }
                                            },
                                          ),
                                          onTap: () {
                                            widget.appState.selectConversation(conv.id);
                                            Navigator.pop(context);
                                          },
                                        ),
                                      );
                                    }),
                                  ],
                                );
                              },
                            )
                  : const SizedBox(),
            ),

            const Divider(color: AppTheme.darkDivider, height: 1),

            // Pinned Account Section at the Bottom
            if (user != null)
              InkWell(
                onTap: () {
                  AccountPopup.show(
                    context,
                    user: user,
                    onSwitchAccount: widget.onSwitchAccount,
                    onPersonalization: widget.onOpenPersonalization,
                    onSignOut: widget.onSignOut,
                    onAuthorizeDrive: () => widget.appState.authorizeDrive(),
                  );
                },
                child: Container(
                  padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
                  child: Row(
                    children: [
                      CircleAvatar(
                        radius: 18,
                        backgroundColor: AppTheme.darkSurfaceVariant,
                        backgroundImage: user.photoUrl != null
                            ? NetworkImage(user.photoUrl!)
                            : null,
                        child: user.photoUrl == null
                            ? Text(
                                user.name.isNotEmpty ? user.name[0].toUpperCase() : 'G',
                                style: const TextStyle(
                                  fontSize: 14,
                                  fontWeight: FontWeight.bold,
                                  color: AppTheme.textPrimaryDark,
                                ),
                              )
                            : null,
                      ),
                      const SizedBox(width: 10),
                      Expanded(
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              user.name,
                              style: const TextStyle(
                                fontSize: 13,
                                fontWeight: FontWeight.w600,
                                color: AppTheme.textPrimaryDark,
                              ),
                              overflow: TextOverflow.ellipsis,
                            ),
                            Text(
                              user.email,
                              style: const TextStyle(
                                fontSize: 11,
                                color: AppTheme.textMutedDark,
                              ),
                              overflow: TextOverflow.ellipsis,
                            ),
                          ],
                        ),
                      ),
                      const Icon(Icons.more_horiz, size: 18, color: AppTheme.textMutedDark),
                    ],
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

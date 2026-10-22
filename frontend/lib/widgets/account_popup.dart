import 'package:flutter/material.dart';
import '../models/auth_user.dart';
import '../theme/app_theme.dart';

/// Compact account popup opened from the bottom account section of the sidebar.
class AccountPopup extends StatelessWidget {
  final AuthUser user;
  final VoidCallback onSwitchAccount;
  final VoidCallback onPersonalization;
  final VoidCallback onSignOut;
  final VoidCallback? onAuthorizeDrive;

  const AccountPopup({
    super.key,
    required this.user,
    required this.onSwitchAccount,
    required this.onPersonalization,
    required this.onSignOut,
    this.onAuthorizeDrive,
  });

  static void show(
    BuildContext context, {
    required AuthUser user,
    required VoidCallback onSwitchAccount,
    required VoidCallback onPersonalization,
    required VoidCallback onSignOut,
    VoidCallback? onAuthorizeDrive,
  }) {
    showModalBottomSheet(
      context: context,
      backgroundColor: AppTheme.darkSurface,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(20)),
      ),
      builder: (ctx) => AccountPopup(
        user: user,
        onSwitchAccount: () {
          Navigator.pop(ctx);
          onSwitchAccount();
        },
        onPersonalization: () {
          Navigator.pop(ctx);
          onPersonalization();
        },
        onSignOut: () {
          Navigator.pop(ctx);
          onSignOut();
        },
        onAuthorizeDrive: onAuthorizeDrive != null
            ? () {
                Navigator.pop(ctx);
                onAuthorizeDrive();
              }
            : null,
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return SafeArea(
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 16),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Center(
              child: Container(
                width: 36,
                height: 4,
                margin: const EdgeInsets.only(bottom: 16),
                decoration: BoxDecoration(
                  color: AppTheme.darkBorder,
                  borderRadius: BorderRadius.circular(2),
                ),
              ),
            ),
            // User identity section
            Row(
              children: [
                CircleAvatar(
                  radius: 22,
                  backgroundColor: AppTheme.darkSurfaceVariant,
                  backgroundImage: user.photoUrl != null
                      ? NetworkImage(user.photoUrl!)
                      : null,
                  child: user.photoUrl == null
                      ? Text(
                          user.name.isNotEmpty ? user.name[0].toUpperCase() : 'G',
                          style: const TextStyle(
                            fontSize: 16,
                            fontWeight: FontWeight.bold,
                            color: AppTheme.textPrimaryDark,
                          ),
                        )
                      : null,
                ),
                const SizedBox(width: 12),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        user.name,
                        style: const TextStyle(
                          fontSize: 15,
                          fontWeight: FontWeight.w600,
                          color: AppTheme.textPrimaryDark,
                        ),
                      ),
                      const SizedBox(height: 2),
                      Text(
                        user.email,
                        style: const TextStyle(
                          fontSize: 12,
                          color: AppTheme.textSecondaryDark,
                        ),
                        overflow: TextOverflow.ellipsis,
                      ),
                      const SizedBox(height: 2),
                      Row(
                        children: [
                          Container(
                            width: 6,
                            height: 6,
                            decoration: BoxDecoration(
                              color: user.hasDriveToken ? AppTheme.statusSuccess : AppTheme.accentGreen,
                              shape: BoxShape.circle,
                            ),
                          ),
                          const SizedBox(width: 4),
                          Text(
                            user.hasDriveToken ? 'Google Drive Connected' : 'Google Drive Authorization Needed',
                            style: const TextStyle(
                              fontSize: 11,
                              color: AppTheme.textMutedDark,
                            ),
                          ),
                        ],
                      ),
                    ],
                  ),
                ),
              ],
            ),
            const SizedBox(height: 16),
            const Divider(color: AppTheme.darkDivider),
            const SizedBox(height: 8),

            // Actions
            if (!user.hasDriveToken && onAuthorizeDrive != null) ...[
              ListTile(
                leading: const Icon(
                  Icons.cloud_upload_outlined,
                  color: AppTheme.accentGreen,
                  size: 20,
                ),
                title: const Text(
                  'Authorize Google Drive',
                  style: TextStyle(fontSize: 14, color: AppTheme.accentGreen, fontWeight: FontWeight.w500),
                ),
                onTap: onAuthorizeDrive,
              ),
            ],
            ListTile(
              leading: const Icon(
                Icons.person_add_alt_1_outlined,
                color: AppTheme.textPrimaryDark,
                size: 20,
              ),
              title: const Text(
                'Switch Google Account',
                style: TextStyle(fontSize: 14, color: AppTheme.textPrimaryDark),
              ),
              onTap: onSwitchAccount,
            ),
            ListTile(
              leading: const Icon(
                Icons.tune_outlined,
                color: AppTheme.textPrimaryDark,
                size: 20,
              ),
              title: const Text(
                'Personalization',
                style: TextStyle(fontSize: 14, color: AppTheme.textPrimaryDark),
              ),
              onTap: onPersonalization,
            ),
            ListTile(
              leading: const Icon(
                Icons.logout,
                color: AppTheme.statusError,
                size: 20,
              ),
              title: const Text(
                'Sign Out',
                style: TextStyle(fontSize: 14, color: AppTheme.statusError),
              ),
              onTap: onSignOut,
            ),
            const SizedBox(height: 8),
          ],
        ),
      ),
    );
  }
}

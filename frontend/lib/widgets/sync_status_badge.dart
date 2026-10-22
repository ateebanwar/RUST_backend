import 'package:flutter/material.dart';
import '../models/drive_status.dart';
import '../theme/app_theme.dart';

/// Compact status badge indicating Google Drive synchronization state.
class SyncStatusBadge extends StatelessWidget {
  final SyncStatus? status;
  final bool isSyncing;
  final bool hasDriveToken;
  final String? errorMessage;
  final VoidCallback? onSyncTap;

  const SyncStatusBadge({
    super.key,
    this.status,
    this.isSyncing = false,
    this.hasDriveToken = true,
    this.errorMessage,
    this.onSyncTap,
  });

  @override
  Widget build(BuildContext context) {
    if (isSyncing) {
      return Container(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
        decoration: BoxDecoration(
          color: AppTheme.darkSurfaceVariant,
          borderRadius: BorderRadius.circular(12),
        ),
        child: const Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            SizedBox(
              width: 12,
              height: 12,
              child: CircularProgressIndicator(
                strokeWidth: 1.5,
                color: AppTheme.accentGreen,
              ),
            ),
            SizedBox(width: 6),
            Text(
              'Syncing...',
              style: TextStyle(fontSize: 11, color: AppTheme.textSecondaryDark),
            ),
          ],
        ),
      );
    }

    if (!hasDriveToken) {
      return InkWell(
        onTap: onSyncTap,
        borderRadius: BorderRadius.circular(12),
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
          decoration: BoxDecoration(
            color: AppTheme.accentGreen.withOpacity(0.12),
            borderRadius: BorderRadius.circular(12),
            border: Border.all(color: AppTheme.accentGreen.withOpacity(0.3), width: 1),
          ),
          child: const Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(Icons.cloud_upload_outlined, size: 12, color: AppTheme.accentGreen),
              SizedBox(width: 4),
              Text(
                'Connect Drive',
                style: TextStyle(fontSize: 11, color: AppTheme.accentGreen, fontWeight: FontWeight.w500),
              ),
            ],
          ),
        ),
      );
    }

    if (errorMessage != null || status?.lastError != null) {
      return InkWell(
        onTap: onSyncTap,
        borderRadius: BorderRadius.circular(12),
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
          decoration: BoxDecoration(
            color: AppTheme.statusError.withOpacity(0.15),
            borderRadius: BorderRadius.circular(12),
            border: Border.all(color: AppTheme.statusError.withOpacity(0.3), width: 1),
          ),
          child: const Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(Icons.cloud_off, size: 12, color: AppTheme.statusError),
              SizedBox(width: 4),
              Text(
                'Save failed',
                style: TextStyle(fontSize: 11, color: AppTheme.statusError, fontWeight: FontWeight.w500),
              ),
            ],
          ),
        ),
      );
    }

    if (status != null && status!.syncEnabled) {
      return InkWell(
        onTap: onSyncTap,
        borderRadius: BorderRadius.circular(12),
        child: Container(
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
          decoration: BoxDecoration(
            color: AppTheme.statusSuccess.withOpacity(0.12),
            borderRadius: BorderRadius.circular(12),
          ),
          child: const Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(Icons.cloud_done, size: 12, color: AppTheme.statusSuccess),
              SizedBox(width: 4),
              Text(
                'Synced',
                style: TextStyle(fontSize: 11, color: AppTheme.statusSuccess, fontWeight: FontWeight.w500),
              ),
            ],
          ),
        ),
      );
    }

    return InkWell(
      onTap: onSyncTap,
      borderRadius: BorderRadius.circular(12),
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
        decoration: BoxDecoration(
          color: AppTheme.darkSurfaceVariant,
          borderRadius: BorderRadius.circular(12),
        ),
        child: const Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.cloud_queue, size: 12, color: AppTheme.textSecondaryDark),
            SizedBox(width: 4),
            Text(
              'Drive ready',
              style: TextStyle(fontSize: 11, color: AppTheme.textSecondaryDark),
            ),
          ],
        ),
      ),
    );
  }
}

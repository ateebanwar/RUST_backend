/// Represents an authenticated user session.
/// 
/// Contains OIDC profile data, the Google ID token used for backend
/// authentication (`Authorization: Bearer <idToken>`), and the Google Drive
/// access token used for AppData persistence (`X-Drive-Token: <token>`).
class AuthUser {
  final String id;
  final String name;
  final String email;
  final String? photoUrl;
  final String idToken;
  final String? driveAccessToken;
  final bool isMock;

  const AuthUser({
    required this.id,
    required this.name,
    required this.email,
    this.photoUrl,
    required this.idToken,
    this.driveAccessToken,
    this.isMock = false,
  });

  bool get hasDriveToken =>
      driveAccessToken != null && driveAccessToken!.trim().isNotEmpty;

  AuthUser copyWith({
    String? id,
    String? name,
    String? email,
    String? photoUrl,
    String? idToken,
    String? driveAccessToken,
    bool? isMock,
  }) {
    return AuthUser(
      id: id ?? this.id,
      name: name ?? this.name,
      email: email ?? this.email,
      photoUrl: photoUrl ?? this.photoUrl,
      idToken: idToken ?? this.idToken,
      driveAccessToken: driveAccessToken ?? this.driveAccessToken,
      isMock: isMock ?? this.isMock,
    );
  }

  Map<String, dynamic> toJson() => {
        'id': id,
        'name': name,
        'email': email,
        'photoUrl': photoUrl,
      };
}
